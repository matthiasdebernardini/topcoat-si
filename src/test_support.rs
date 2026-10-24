#![cfg(test)]
//! Test helpers: a local fake HTTP server (one canned response per connection, every request
//! recorded) and a recorder for the warnings logged on unknown API values.

use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::subscriber::DefaultGuard;
use tracing::{field, span};

/// One canned response: status, extra headers, body, content type, an optional stall and whether
/// the body is cut short.
#[derive(Debug, Clone)]
pub(crate) struct Canned {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
    content_type: String,
    stall: Option<Duration>,
    cut: bool,
    close_unanswered: bool,
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
            cut: false,
            close_unanswered: false,
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

    /// Reads the request, then closes the connection without any reply.
    pub(crate) fn closing_unanswered(mut self) -> Self {
        self.close_unanswered = true;
        self
    }

    /// Announces a longer body than it sends, then closes, so the client's body read fails.
    pub(crate) fn with_cut_body(mut self) -> Self {
        self.cut = true;
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
            if canned.close_unanswered {
                continue;
            }
            let extra: String = canned
                .headers
                .iter()
                .map(|(name, value)| format!("{name}: {value}\r\n"))
                .collect();
            let head = format!(
                "HTTP/1.1 {} X\r\nContent-Type: {}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
                canned.status,
                canned.content_type,
                canned.body.len() + if canned.cut { 100 } else { 0 }
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

/// One event logged on a recorded target: its level, message and other fields as `field=value`
/// pairs.
#[derive(Debug, Clone)]
pub(crate) struct Logged {
    pub(crate) level: tracing::Level,
    pub(crate) message: String,
    pub(crate) fields: String,
}

/// Records every event logged on one target.
struct Events(&'static str, Arc<Mutex<Vec<Logged>>>);

impl tracing::Subscriber for Events {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &span::Attributes<'_>) -> span::Id {
        span::Id::from_u64(1)
    }

    fn record(&self, _span: &span::Id, _values: &span::Record<'_>) {}

    fn record_follows_from(&self, _span: &span::Id, _follows: &span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let metadata = event.metadata();
        if metadata.target() != self.0 {
            return;
        }
        #[derive(Default)]
        struct Fields {
            message: String,
            fields: Vec<String>,
        }
        impl field::Visit for Fields {
            fn record_debug(&mut self, field: &field::Field, value: &dyn fmt::Debug) {
                if field.name() == "message" {
                    self.message = format!("{value:?}");
                } else {
                    self.fields.push(format!("{}={value:?}", field.name()));
                }
            }
            fn record_str(&mut self, field: &field::Field, value: &str) {
                self.fields.push(format!("{}={value}", field.name()));
            }
        }
        let mut fields = Fields::default();
        event.record(&mut fields);
        self.1.lock().unwrap().push(Logged {
            level: *metadata.level(),
            message: fields.message,
            fields: fields.fields.join(" "),
        });
    }

    fn enter(&self, _span: &span::Id) {}

    fn exit(&self, _span: &span::Id) {}
}

/// Runs `f` and returns its result with every unknown-value warning it logged on this thread,
/// each as `rust_type=... tag=...`.
pub(crate) fn unknown_warnings<R>(f: impl FnOnce() -> R) -> (R, Vec<String>) {
    warnings_on("carpa::unknown", f)
}

/// Runs `f` and returns its result with every warning it logged on `target` on this thread,
/// each as `field=value` pairs.
pub(crate) fn warnings_on<R>(target: &'static str, f: impl FnOnce() -> R) -> (R, Vec<String>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let result = tracing::subscriber::with_default(Events(target, events.clone()), f);
    let warnings = events
        .lock()
        .unwrap()
        .iter()
        .filter(|logged| logged.level == tracing::Level::WARN)
        .map(|logged| logged.fields.clone())
        .collect();
    (result, warnings)
}

/// Records the events logged on `target` on this thread until the guard drops. Use it in a
/// current-thread test so every await stays on this thread.
pub(crate) fn record_events(target: &'static str) -> (DefaultGuard, Arc<Mutex<Vec<Logged>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let guard = tracing::subscriber::set_default(Events(target, events.clone()));
    (guard, events)
}

/// Records the target of every span and event: `span <name> <target>` or `event <target>`.
struct Targets(Arc<Mutex<Vec<String>>>);

impl tracing::Subscriber for Targets {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, span: &span::Attributes<'_>) -> span::Id {
        let metadata = span.metadata();
        self.0
            .lock()
            .unwrap()
            .push(format!("span {} {}", metadata.name(), metadata.target()));
        span::Id::from_u64(1)
    }

    fn record(&self, _span: &span::Id, _values: &span::Record<'_>) {}

    fn record_follows_from(&self, _span: &span::Id, _follows: &span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        self.0
            .lock()
            .unwrap()
            .push(format!("event {}", event.metadata().target()));
    }

    fn enter(&self, _span: &span::Id) {}

    fn exit(&self, _span: &span::Id) {}
}

/// Records the targets of the spans and events on this thread until the guard drops. Use it in
/// a current-thread test so every await stays on this thread.
pub(crate) fn record_targets() -> (DefaultGuard, Arc<Mutex<Vec<String>>>) {
    let targets = Arc::new(Mutex::new(Vec::new()));
    let guard = tracing::subscriber::set_default(Targets(targets.clone()));
    (guard, targets)
}

/// The URL-encoded query string of `pairs`.
pub(crate) fn encode_query(pairs: Vec<(String, String)>) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish()
}

/// Whether `client` consults any proxy, system or configured.
///
/// reqwest 0.13's `ClientBuilder::build` always adds the system proxy matcher unless
/// `no_proxy()` was called, and the client's `Debug` output prints a `proxies` field only when
/// its proxy list is nonempty. So the Debug text tells whether a client may use a proxy without
/// touching any environment variable. A reqwest upgrade that changes this Debug output makes
/// the tests that use this helper fail loudly, not pass silently.
pub(crate) fn uses_proxies(client: &reqwest::Client) -> bool {
    format!("{client:?}").contains("proxies")
}
