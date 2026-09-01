use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use futures::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::{
    sync::{Mutex, broadcast, mpsc, oneshot},
    time::{Instant, interval, timeout},
};
use tokio_tungstenite::tungstenite::Message;
use tracing::instrument;
use webtest_browser::BrowserError;

use crate::wire::{Command, IncomingMessage, bounded_text};

const CORRELATION_SWEEP: Duration = Duration::from_millis(10);

#[derive(Clone)]
pub(crate) struct CdpConnection {
    sender: mpsc::Sender<OutgoingCommand>,
    command_timeout: Duration,
    in_flight: Arc<AtomicUsize>,
    console_errors: Arc<Mutex<Vec<String>>>,
    events: broadcast::Sender<CdpEvent>,
}

#[derive(Clone, Debug)]
pub(crate) struct CdpEvent {
    pub(crate) session_id: Option<String>,
    pub(crate) method: String,
    pub(crate) params: Value,
    pub(crate) terminal: Option<BrowserError>,
}

struct OutgoingCommand {
    method: String,
    params: Option<Value>,
    session_id: Option<String>,
    response: oneshot::Sender<Result<Value, BrowserError>>,
    deadline: Instant,
    timeout: Duration,
}

struct PendingCommand {
    method: String,
    session_id: Option<String>,
    response: oneshot::Sender<Result<Value, BrowserError>>,
    deadline: Instant,
    timeout: Duration,
}

impl CdpConnection {
    pub(crate) async fn connect(
        url: &str,
        command_timeout: Duration,
    ) -> Result<Self, BrowserError> {
        let (socket, _) = tokio_tungstenite::connect_async(url)
            .await
            .map_err(|error| {
                BrowserError::Launch(format!("could not connect to Chrome: {error}"))
            })?;
        let (mut writer, mut reader) = socket.split();
        let (sender, mut receiver) = mpsc::channel::<OutgoingCommand>(32);
        let in_flight = Arc::new(AtomicUsize::new(0));
        let actor_in_flight = Arc::clone(&in_flight);
        let console_errors = Arc::new(Mutex::new(Vec::new()));
        let actor_console_errors = Arc::clone(&console_errors);
        let (events, _) = broadcast::channel(256);
        let actor_events = events.clone();

        tokio::spawn(async move {
            let mut next_id = 1u64;
            let mut pending = HashMap::<u64, PendingCommand>::new();
            let mut sweep = interval(CORRELATION_SWEEP);
            let terminal = loop {
                tokio::select! {
                    outgoing = receiver.recv() => {
                        let Some(outgoing) = outgoing else {
                            break BrowserError::BrowserDisconnected;
                        };
                        let id = next_id;
                        next_id = next_id.wrapping_add(1).max(1);
                        let command = Command {
                            id,
                            method: &outgoing.method,
                            params: outgoing.params.as_ref(),
                            session_id: outgoing.session_id.as_deref(),
                        };
                        let encoded = match serde_json::to_string(&command) {
                            Ok(encoded) => encoded,
                            Err(error) => {
                                let _ = outgoing.response.send(Err(BrowserError::Protocol {
                                    method: outgoing.method,
                                    message: error.to_string(),
                                }));
                                continue;
                            }
                        };
                        let method = outgoing.method.clone();
                        let session_id = outgoing.session_id.clone();
                        pending.insert(id, PendingCommand {
                            method,
                            session_id,
                            response: outgoing.response,
                            deadline: outgoing.deadline,
                            timeout: outgoing.timeout,
                        });
                        actor_in_flight.fetch_add(1, Ordering::Relaxed);
                        let remaining = outgoing.deadline.saturating_duration_since(Instant::now());
                        match timeout(remaining, writer.send(Message::Text(encoded.into()))).await {
                            Ok(Ok(())) => {}
                            Ok(Err(_)) => break BrowserError::BrowserDisconnected,
                            Err(_) => {
                                if let Some(command) = pending.remove(&id) {
                                    actor_in_flight.fetch_sub(1, Ordering::Relaxed);
                                    let _ = command.response.send(Err(BrowserError::CommandTimeout {
                                        method: command.method,
                                        timeout_ms: duration_millis(command.timeout),
                                    }));
                                }
                            }
                        }
                    }
                    incoming = reader.next() => {
                        let message = match incoming {
                            Some(Ok(message)) => message,
                            Some(Err(_)) | None => break BrowserError::BrowserDisconnected,
                        };
                        let text = match message {
                            Message::Text(text) => text,
                            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
                            Message::Close(_) => break BrowserError::BrowserDisconnected,
                            Message::Binary(_) => break BrowserError::MalformedProtocol {
                                message: "Chrome sent a binary CDP response".into(),
                            },
                        };
                        let message = match serde_json::from_str::<IncomingMessage>(&text) {
                            Ok(message) => message,
                            Err(error) => break BrowserError::MalformedProtocol {
                                message: error.to_string(),
                            },
                        };
                        let Some(id) = message.id else {
                            if let Some(entry) = console_error(&message) {
                                let mut errors = actor_console_errors.lock().await;
                                if errors.len() == 20 { errors.remove(0); }
                                errors.push(entry);
                            }
                            if let (Some(method), Some(params)) = (message.method, message.params) {
                                let _ = actor_events.send(CdpEvent {
                                    session_id: message.session_id,
                                    method,
                                    params,
                                    terminal: None,
                                });
                            }
                            continue
                        };
                        let Some(pending_command) = pending.remove(&id) else {
                            tracing::warn!(id, "Chrome returned a response for an unknown CDP command");
                            continue;
                        };
                        actor_in_flight.fetch_sub(1, Ordering::Relaxed);
                        let result = if message.session_id != pending_command.session_id {
                            Err(BrowserError::MalformedProtocol {
                                message: format!(
                                    "response {id} used session {:?}, expected {:?}",
                                    message.session_id, pending_command.session_id
                                ),
                            })
                        } else if let Some(error) = message.error {
                            Err(BrowserError::Protocol {
                                method: pending_command.method,
                                message: format!("{} ({})", error.message, error.code),
                            })
                        } else if let Some(result) = message.result {
                            Ok(result)
                        } else {
                            Err(BrowserError::MalformedProtocol {
                                message: format!("response {id} contained neither result nor error"),
                            })
                        };
                        let _ = pending_command.response.send(result);
                    }
                    _ = sweep.tick() => {
                        let now = Instant::now();
                        let expired = pending
                            .iter()
                            .filter_map(|(id, command)| {
                                (command.deadline <= now || command.response.is_closed()).then_some(*id)
                            })
                            .collect::<Vec<_>>();
                        for id in expired {
                            let Some(command) = pending.remove(&id) else { continue };
                            actor_in_flight.fetch_sub(1, Ordering::Relaxed);
                            if !command.response.is_closed() {
                                let _ = command.response.send(Err(BrowserError::CommandTimeout {
                                    method: command.method,
                                    timeout_ms: duration_millis(command.timeout),
                                }));
                            }
                        }
                    }
                }
            };
            let _ = actor_events.send(CdpEvent {
                session_id: None,
                method: String::new(),
                params: Value::Null,
                terminal: Some(terminal.clone()),
            });
            for (_, command) in pending {
                actor_in_flight.fetch_sub(1, Ordering::Relaxed);
                let _ = command.response.send(Err(terminal.clone()));
            }
        });

        Ok(Self {
            sender,
            command_timeout,
            in_flight,
            console_errors,
            events,
        })
    }

    #[instrument(skip_all, fields(method))]
    pub(crate) async fn command(
        &self,
        method: &str,
        params: Option<Value>,
        session_id: Option<&str>,
    ) -> Result<Value, BrowserError> {
        self.command_with_timeout(method, params, session_id, self.command_timeout)
            .await
    }

    #[instrument(skip_all, fields(method))]
    pub(crate) async fn command_with_timeout(
        &self,
        method: &str,
        params: Option<Value>,
        session_id: Option<&str>,
        maximum: Duration,
    ) -> Result<Value, BrowserError> {
        let timeout = self.command_timeout.min(maximum);
        let deadline = Instant::now() + timeout;
        let (response, receive) = oneshot::channel();
        tokio::time::timeout_at(
            deadline,
            self.sender.send(OutgoingCommand {
                method: method.to_owned(),
                params,
                session_id: session_id.map(str::to_owned),
                response,
                deadline,
                timeout,
            }),
        )
        .await
        .map_err(|_| BrowserError::CommandTimeout {
            method: method.to_owned(),
            timeout_ms: duration_millis(timeout),
        })?
        .map_err(|_| BrowserError::BrowserDisconnected)?;
        let result = receive
            .await
            .map_err(|_| BrowserError::BrowserDisconnected)?;
        tracing::trace!(
            pending_commands = self.in_flight.load(Ordering::Relaxed),
            "completed CDP command"
        );
        result
    }

    pub(crate) const fn command_timeout(&self) -> Duration {
        self.command_timeout
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<CdpEvent> {
        self.events.subscribe()
    }

    #[cfg(test)]
    pub(crate) fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::Relaxed)
    }

    pub(crate) async fn console_errors(&self) -> Vec<String> {
        self.console_errors.lock().await.clone()
    }
}

fn console_error(message: &IncomingMessage) -> Option<String> {
    match message.method.as_deref()? {
        "Runtime.exceptionThrown" => message
            .params
            .as_ref()?
            .pointer("/exceptionDetails/text")
            .and_then(Value::as_str)
            .map(bounded_text),
        "Runtime.consoleAPICalled"
            if message.params.as_ref()?.get("type").and_then(Value::as_str) == Some("error") =>
        {
            Some("console.error".into())
        }
        _ => None,
    }
}

fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests;
