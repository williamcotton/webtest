use crate::{error::AppError, report::ExitClass};
use futures::{StreamExt, stream::FuturesUnordered};
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use webtest_trace::{TraceLimits, VIEWER_CSP};

const MAX_HTML_BYTES: usize = 64 * 1024 * 1024;
const MAX_REQUEST_BYTES: usize = 8192;
const MAX_CONNECTIONS: usize = 16;
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) async fn run(artifact: PathBuf, output: Option<PathBuf>) -> Result<ExitClass, AppError> {
    let bytes = read_archive(&artifact)?;
    let trace = webtest_trace::read(&bytes, TraceLimits::default()).map_err(AppError::usage)?;
    let html = webtest_trace::render_html(&trace, MAX_HTML_BYTES).map_err(AppError::usage)?;
    if let Some(output) = output {
        publish_html(&output, html.as_bytes())?;
        println!("wrote trace viewer {}", output.display());
        return Ok(ExitClass::Success);
    }
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(AppError::infrastructure)?;
    let address = listener.local_addr().map_err(AppError::infrastructure)?;
    let mut secret = [0u8; 16];
    getrandom::fill(&mut secret).map_err(AppError::infrastructure)?;
    let path = format!("/{:032x}", u128::from_be_bytes(secret));
    println!("trace viewer: http://{address}{path}");
    std::io::stdout()
        .flush()
        .map_err(AppError::infrastructure)?;
    serve(
        listener,
        address,
        path,
        Arc::new(html),
        tokio::signal::ctrl_c(),
    )
    .await?;
    Ok(ExitClass::Success)
}

fn read_archive(path: &Path) -> Result<Vec<u8>, AppError> {
    let metadata = std::fs::symlink_metadata(path).map_err(AppError::infrastructure)?;
    if !metadata.is_file() {
        return Err(AppError::usage(
            "trace input must be a regular file, not a link or directory",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let file = options.open(path).map_err(AppError::infrastructure)?;
    let metadata = file.metadata().map_err(AppError::infrastructure)?;
    if !metadata.is_file() {
        return Err(AppError::usage("trace input must be a regular file"));
    }
    let maximum = TraceLimits::default().max_archive_bytes;
    let too_large = || AppError::usage(format!("trace archive exceeds its {maximum}-byte limit"));
    if metadata.len() > maximum as u64 {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    file.take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(AppError::infrastructure)?;
    if bytes.len() > maximum {
        return Err(too_large());
    }
    Ok(bytes)
}

fn publish_html(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(AppError::infrastructure)?;
    temp.write_all(bytes).map_err(AppError::infrastructure)?;
    temp.as_file()
        .sync_all()
        .map_err(AppError::infrastructure)?;
    temp.persist_noclobber(path)
        .map_err(AppError::infrastructure)?;
    Ok(())
}

async fn serve(
    listener: TcpListener,
    address: SocketAddr,
    path: String,
    html: Arc<String>,
    shutdown: impl std::future::Future<Output = std::io::Result<()>>,
) -> Result<(), AppError> {
    let mut requests = FuturesUnordered::new();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            biased;
            result = &mut shutdown => { result.map_err(AppError::infrastructure)?; break; }
            Some(()) = requests.next(), if !requests.is_empty() => {},
            connection = listener.accept(), if requests.len() < MAX_CONNECTIONS => {
                let (stream, _) = connection.map_err(AppError::infrastructure)?;
                requests.push(connection_task(stream, address, path.clone(), html.clone()));
            }
        }
    }
    // Futures own their sockets directly. Dropping them closes all connections;
    // there are no spawned tasks or detached work to outlive this server.
    drop(requests);
    Ok(())
}

async fn connection_task(
    mut stream: TcpStream,
    address: SocketAddr,
    path: String,
    html: Arc<String>,
) {
    let _ = tokio::time::timeout(CONNECTION_TIMEOUT, async {
        let mut request = Vec::new();
        let mut chunk = [0u8; 1024];
        let status = loop {
            let count = stream.read(&mut chunk).await?;
            if count == 0 { return Ok::<_, std::io::Error>(()); }
            request.extend_from_slice(&chunk[..count]);
            if request.len() > MAX_REQUEST_BYTES { break 431; }
            if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                break request_status(&request[..end], address, &path);
            }
        };
        let head = request.starts_with(b"HEAD ");
        let (reason, body) = match status {
            200 => ("OK", html.as_str()), 403 => ("Forbidden", "Forbidden"),
            404 => ("Not Found", "Not found"), 405 => ("Method Not Allowed", "Read-only viewer"),
            431 => ("Request Header Fields Too Large", "Request too large"), _ => ("Bad Request", "Invalid request"),
        };
        let header = format!("HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: {VIEWER_CSP}; frame-ancestors 'none'\r\n\r\n", body.len());
        stream.write_all(header.as_bytes()).await?;
        if !head { stream.write_all(body.as_bytes()).await?; }
        stream.shutdown().await
    }).await;
}

fn request_status(request: &[u8], address: SocketAddr, path: &str) -> u16 {
    let Ok(text) = std::str::from_utf8(request) else {
        return 400;
    };
    let mut lines = text.split("\r\n");
    let Some(first) = lines.next() else {
        return 400;
    };
    let parts: Vec<_> = first.split(' ').collect();
    if parts.len() != 3 || !matches!(parts[2], "HTTP/1.1" | "HTTP/1.0") {
        return 400;
    }
    if !matches!(parts[0], "GET" | "HEAD") {
        return 405;
    }
    let mut host = None;
    let mut origin = None;
    for line in lines {
        let Some((key, value)) = line.split_once(':') else {
            return 400;
        };
        if key.eq_ignore_ascii_case("host") && host.replace(value.trim()).is_some() {
            return 400;
        }
        if key.eq_ignore_ascii_case("origin") && origin.replace(value.trim()).is_some() {
            return 400;
        }
        if key.eq_ignore_ascii_case("transfer-encoding")
            || (key.eq_ignore_ascii_case("content-length") && value.trim() != "0")
        {
            return 400;
        }
    }
    if host != Some(address.to_string().as_str())
        || origin.is_some_and(|value| value != format!("http://{address}"))
    {
        return 403;
    }
    if parts[1] != path {
        return 404;
    }
    200
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_gate_rejects_foreign_hosts_origins_writes_and_nonviewer_paths() {
        let address: SocketAddr = "127.0.0.1:1234".parse().unwrap();
        for (request, expected) in [
            ("GET /token HTTP/1.1\r\nHost: 127.0.0.1:1234", 200),
            ("HEAD /token HTTP/1.1\r\nhost: 127.0.0.1:1234", 200),
            ("GET /token HTTP/1.1\r\nHost: evil.invalid", 403),
            (
                "GET /token HTTP/1.1\r\nHost: 127.0.0.1:1234\r\nOrigin: https://evil.invalid",
                403,
            ),
            ("GET /token HTTP/1.1", 403),
            (
                "GET /token HTTP/1.1\r\nHost: 127.0.0.1:1234\r\nHost: evil.invalid",
                400,
            ),
            ("POST /token HTTP/1.1\r\nHost: 127.0.0.1:1234", 405),
            (
                "GET /token HTTP/1.1\r\nHost: 127.0.0.1:1234\r\nContent-Length: 5",
                400,
            ),
            (
                "GET /token HTTP/1.1\r\nHost: 127.0.0.1:1234\r\nTransfer-Encoding: chunked",
                400,
            ),
            (
                "GET /token/../manifest.json HTTP/1.1\r\nHost: 127.0.0.1:1234",
                404,
            ),
            (
                "GET /token?file=/etc/passwd HTTP/1.1\r\nHost: 127.0.0.1:1234",
                404,
            ),
            ("GET / HTTP/1.1\r\nHost: 127.0.0.1:1234", 404),
        ] {
            assert_eq!(
                request_status(request.as_bytes(), address, "/token"),
                expected,
                "{request}"
            );
        }
    }

    #[tokio::test]
    async fn viewer_serves_immutable_html_and_shutdown_closes_stalled_requests() {
        let listener = match TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::AddrNotAvailable
                ) =>
            {
                return;
            }
            Err(error) => panic!("loopback: {error}"),
        };
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(serve(
            listener,
            address,
            "/token".into(),
            Arc::new("<h1>cached trace</h1>".into()),
            async {
                stopped.await.unwrap();
                Ok(())
            },
        ));
        let mut stalled = TcpStream::connect(address).await.unwrap();
        stalled.write_all(b"GET /token HTTP/1.1\r\n").await.unwrap();
        for method in ["GET", "HEAD"] {
            let mut stream = TcpStream::connect(address).await.unwrap();
            stream
                .write_all(
                    format!("{method} /token HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes(),
                )
                .await
                .unwrap();
            let mut response = String::new();
            tokio::time::timeout(Duration::from_secs(2), stream.read_to_string(&mut response))
                .await
                .unwrap()
                .unwrap();
            assert!(response.starts_with("HTTP/1.1 200 OK"));
            assert!(response.contains("default-src 'none'"));
            assert!(response.contains("frame-ancestors 'none'"));
            assert!(response.contains("Cache-Control: no-store"));
            assert_eq!(response.contains("<h1>cached trace</h1>"), method == "GET");
        }
        stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let mut byte = [0u8; 1];
        let result = tokio::time::timeout(Duration::from_secs(2), stalled.read(&mut byte))
            .await
            .unwrap();
        assert!(matches!(result, Ok(0) | Err(_)));
        assert!(TcpStream::connect(address).await.is_err());
    }

    #[test]
    fn html_publication_is_nonoverwriting_and_failed_writes_leave_no_temporary_file() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("trace.html");
        publish_html(&output, b"first").unwrap();
        assert!(publish_html(&output, b"second").is_err());
        assert_eq!(std::fs::read(&output).unwrap(), b"first");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
