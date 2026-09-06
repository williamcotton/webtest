use std::{io, net::SocketAddr, thread, time::Duration};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::oneshot,
    task::JoinSet,
};

/// An owned HTTP fixture which also accepts browser preconnections that send no request.
pub struct HttpFixture {
    pub address: SocketAddr,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl HttpFixture {
    pub fn start(status: &str, content_type: &str, body: &str) -> Option<Self> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").ok()?;
        listener.set_nonblocking(true).expect("nonblocking fixture");
        let address = listener.local_addr().expect("fixture address");
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let (stop, mut stopped) = oneshot::channel();
        let thread = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("fixture runtime");
            runtime.block_on(async move {
                let listener =
                    tokio::net::TcpListener::from_std(listener).expect("fixture listener");
                let mut connections = JoinSet::new();
                loop {
                    tokio::select! {
                        biased;
                        _ = &mut stopped => break,
                        Some(_) = connections.join_next() => {},
                        accepted = listener.accept() => {
                            let Ok((stream, _)) = accepted else { break };
                            let response = response.clone();
                            connections.spawn(async move {
                                let _ = tokio::time::timeout(
                                    Duration::from_secs(5),
                                    respond(stream, response),
                                ).await;
                            });
                        }
                    }
                }
                // Shutdown owns every connection, including idle or partial requests.
                connections.abort_all();
                while connections.join_next().await.is_some() {}
            });
        });
        Some(Self {
            address,
            stop: Some(stop),
            thread: Some(thread),
        })
    }
}

async fn respond(mut stream: TcpStream, response: String) -> io::Result<()> {
    let mut request = Vec::new();
    let mut buffer = [0; 2048];
    loop {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            return Ok(());
        }
        request.extend_from_slice(&buffer[..count]);
        if request.len() > 16 * 1024 {
            return Ok(());
        }
        if request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            break;
        }
    }
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await?;
    // Drain until the client closes so unread bytes cannot reset its response.
    while stream.read(&mut buffer).await? != 0 {}
    Ok(())
}

impl Drop for HttpFixture {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
