//! Bounded async HTTP shared by native providers and application adapters.
use std::{collections::BTreeMap, time::Duration};

use webtest_host::OperationContext;

use crate::{ProviderError, ResponseValue, value_from_json};

pub struct HttpClient {
    client: Result<reqwest::Client, String>,
}

pub struct HttpRequest<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub headers: &'a BTreeMap<String, String>,
    pub body: Option<Vec<u8>>,
    pub timeout: Duration,
    pub max_response_bytes: usize,
}

impl HttpClient {
    pub fn new(follow_redirects: bool) -> Self {
        Self {
            client: reqwest::Client::builder()
                .cookie_store(true)
                // A finished/cancelled operation must not retain an idle connection.
                .pool_max_idle_per_host(0)
                .redirect(if follow_redirects {
                    reqwest::redirect::Policy::limited(10)
                } else {
                    reqwest::redirect::Policy::none()
                })
                .build()
                .map_err(|error| error.to_string()),
        }
    }

    /// Readiness depends on response headers, not on a potentially streaming body.
    pub async fn healthy(&self, url: &str, timeout: Duration) -> bool {
        let Ok(client) = &self.client else {
            return false;
        };
        client
            .get(url)
            .timeout(timeout)
            .send()
            .await
            .is_ok_and(|response| response.status().as_u16() < 400)
    }

    pub async fn execute(
        &self,
        input: HttpRequest<'_>,
        context: Option<&dyn OperationContext>,
    ) -> Result<ResponseValue, ProviderError> {
        let operation = async {
            let client = self
                .client
                .as_ref()
                .map_err(|message| ProviderError::HttpTransport {
                    message: message.clone(),
                })?;
            let method =
                reqwest::Method::from_bytes(input.method.as_bytes()).map_err(transport_error)?;
            let mut request = client.request(method, input.url).timeout(input.timeout);
            for (name, value) in input.headers {
                request = request.header(name, value);
            }
            if let Some(body) = input.body {
                request = request.body(body);
            }
            let mut response = request.send().await.map_err(transport_error)?;
            let status = response.status().as_u16();
            let mut headers = BTreeMap::new();
            for (name, value) in response.headers() {
                headers
                    .entry(name.to_string())
                    .or_insert_with(|| String::from_utf8_lossy(value.as_bytes()).into_owned());
            }
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
                if chunk.len() > input.max_response_bytes.saturating_sub(body.len()) {
                    return Err(ProviderError::ResponseTooLarge {
                        limit: input.max_response_bytes,
                    });
                }
                body.extend_from_slice(&chunk);
            }
            let json = serde_json::from_slice(&body)
                .ok()
                .map(value_from_json)
                .map(Box::new);
            Ok(ResponseValue {
                status,
                headers,
                body,
                json,
            })
        };
        let cancelled = async {
            match context {
                Some(context) => context.cancelled().await,
                None => std::future::pending().await,
            }
        };
        // Dropping async network I/O releases the socket; there is no blocking worker
        // left running after the cancellation acknowledgement.
        tokio::select! {
            biased;
            cause = cancelled => Err(ProviderError::Cancelled { cause, cleanup_succeeded: true }),
            result = operation => result,
        }
    }
}

fn transport_error(error: impl std::fmt::Display) -> ProviderError {
    ProviderError::HttpTransport {
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use webtest_host::{Cancellation, CancellationReason, ExecutionScopeId};

    #[derive(Debug)]
    struct CancelAfterRequest {
        ready: tokio::sync::Notify,
        reason: CancellationReason,
    }
    #[async_trait]
    impl OperationContext for CancelAfterRequest {
        fn scope_id(&self) -> ExecutionScopeId {
            ExecutionScopeId(7)
        }
        fn remaining(&self) -> Option<Duration> {
            Some(Duration::from_secs(30))
        }
        fn cancellation(&self) -> Option<Cancellation> {
            None
        }
        async fn cancelled(&self) -> Cancellation {
            self.ready.notified().await;
            Cancellation {
                reason: self.reason,
                causing_scope_id: self.scope_id(),
            }
        }
    }

    #[tokio::test]
    async fn cancellation_closes_header_and_body_waits_with_original_cause() {
        for reason in [
            CancellationReason::RaceLost,
            CancellationReason::ParentFailed,
            CancellationReason::Timeout,
            CancellationReason::FailFast,
            CancellationReason::DebugDisconnect,
        ] {
            for during_body in [false, true] {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let url = format!("http://{}/", listener.local_addr().unwrap());
                let context = CancelAfterRequest {
                    ready: Default::default(),
                    reason,
                };
                let fixture = async {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut bytes = [0; 1024];
                    assert!(socket.read(&mut bytes).await.unwrap() > 0);
                    if during_body {
                        socket
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nx")
                            .await
                            .unwrap();
                    }
                    context.ready.notify_one();
                    let closed = socket.read(&mut bytes).await;
                    // Aborting an unread response can close with FIN or RST.
                    assert!(
                        matches!(closed, Ok(0))
                            || matches!(&closed,
                        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset),
                        "cancelled socket remained open: {reason:?}, body={during_body}: {closed:?}"
                    );
                };
                let client = HttpClient::new(false);
                let headers = BTreeMap::new();
                let operation = client.execute(
                    HttpRequest {
                        method: "GET",
                        url: &url,
                        headers: &headers,
                        body: None,
                        timeout: Duration::from_secs(30),
                        max_response_bytes: 100,
                    },
                    Some(&context),
                );
                let (result, ()) = tokio::time::timeout(Duration::from_secs(3), async {
                    tokio::join!(operation, fixture)
                })
                .await
                .expect("cancellation closed HTTP without waiting for the operation timeout");
                assert!(
                    matches!(result, Err(ProviderError::Cancelled { cause, cleanup_succeeded: true })
                    if cause.reason == reason && cause.causing_scope_id == ExecutionScopeId(7))
                );
            }
        }
    }

    #[tokio::test]
    async fn response_status_and_body_limit_are_preserved() {
        for limit in [2, 3] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/", listener.local_addr().unwrap());
            let fixture = async {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = [0; 1024];
                assert!(socket.read(&mut bytes).await.unwrap() > 0);
                socket.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 3\r\nConnection: close\r\n\r\n404").await.unwrap();
            };
            let client = HttpClient::new(false);
            let headers = BTreeMap::new();
            let (result, ()) = tokio::join!(
                client.execute(
                    HttpRequest {
                        method: "GET",
                        url: &url,
                        headers: &headers,
                        body: None,
                        timeout: Duration::from_secs(3),
                        max_response_bytes: limit
                    },
                    None
                ),
                fixture
            );
            if limit == 2 {
                assert!(matches!(
                    result,
                    Err(ProviderError::ResponseTooLarge { limit: 2 })
                ));
            } else {
                let response = result.unwrap();
                assert_eq!(response.status, 404);
                assert_eq!(response.body, b"404");
            }
        }
    }
}
