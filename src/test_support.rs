//! A local fake HTTP server for tests: one canned response per connection, every request recorded.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// One canned response: status, extra headers, body, content type and an optional stall.
#[derive(Debug, Clone)]
pub(crate) struct Canned {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
    content_type: String,
    stall: Option<Duration>,
}

impl Canned {
    /// A response with this status and body, served as `application/json`.
    pub(crate) fn json(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
            content_type: "application/json".to_string(),
            stall: None,
        }
    }

    /// Adds one extra response header.
    pub(crate) fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Replaces the `Content-Type` header.
    pub(crate) fn with_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = content_type.into();
        self
    }

    /// Sends the status line and headers, waits this long, then sends the body, so a client
    /// timeout must cover the body read to fire.
    pub(crate) fn with_stall(mut self, stall: Duration) -> Self {
        self.stall = Some(stall);
        self
    }
}

/// The head (request line and headers) and body of one request the server received.
pub(crate) struct Recorded {
    pub(crate) headers: String,
    pub(crate) body: String,
}

pub(crate) async fn read_request(socket: &mut TcpStream) -> Recorded {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];
    let headers_end = loop {
        let read = socket.read(&mut chunk).await.unwrap();
        assert!(read > 0, "client closed the connection early");
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break end;
        }
    };
    let headers = String::from_utf8_lossy(&buffer[..headers_end]).to_string();
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    while buffer.len() - (headers_end + 4) < content_length {
        let read = socket.read(&mut chunk).await.unwrap();
        assert!(read > 0, "client closed the connection before the body");
        buffer.extend_from_slice(&chunk[..read]);
    }
    Recorded {
        headers,
        body: String::from_utf8_lossy(&buffer[headers_end + 4..]).to_string(),
    }
}

/// Serves one canned response per connection, in order, and records each request.
pub(crate) async fn serve(responses: Vec<Canned>) -> (String, Arc<Mutex<Vec<Recorded>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let log = recorded.clone();
    tokio::spawn(async move {
        for canned in responses {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            log.lock().unwrap().push(request);
            let extra: String = canned
                .headers
                .iter()
                .map(|(name, value)| format!("{name}: {value}\r\n"))
                .collect();
            let head = format!(
                "HTTP/1.1 {} X\r\nContent-Type: {}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
                canned.status,
                canned.content_type,
                canned.body.len()
            );
            socket.write_all(head.as_bytes()).await.unwrap();
            if let Some(stall) = canned.stall {
                socket.flush().await.unwrap();
                tokio::time::sleep(stall).await;
                // The client may have timed out and hung up during the stall.
                let _ = socket.write_all(canned.body.as_bytes()).await;
                let _ = socket.shutdown().await;
            } else {
                socket.write_all(canned.body.as_bytes()).await.unwrap();
                socket.shutdown().await.unwrap();
            }
        }
    });
    (format!("http://{address}"), recorded)
}
