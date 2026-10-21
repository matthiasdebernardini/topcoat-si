use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use topcoat_si::*;

async fn server(
    first: Vec<u8>,
    second: Vec<u8>,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let attempts = Arc::new(AtomicUsize::new(0));
    let seen = attempts.clone();
    let task = tokio::spawn(async move {
        for response in [first, second] {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buf = [0; 4096];
            loop {
                let n = socket.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                    let len: usize = headers
                        .lines()
                        .find_map(|l| {
                            l.strip_prefix("content-length:")
                                .map(|s| s.trim().parse().unwrap())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + len {
                        break;
                    }
                }
            }
            seen.fetch_add(1, Ordering::SeqCst);
            socket.write_all(&response).await.unwrap();
            socket.shutdown().await.unwrap();
        }
    });
    (url, attempts, task)
}
fn ok_response() -> Vec<u8> {
    let body = r#"{"id":"msg_test","type":"message","role":"assistant","content":[{"type":"text","text":"hello"}],"model":"claude-test","stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}"#;
    format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes()
}
fn client(url: String) -> Anthropic {
    Anthropic::new(Some("dummy-key".into()))
        .unwrap()
        .with_base_url(url)
        .unwrap()
        .with_max_retries(1)
        .with_timeout(Duration::from_secs(2))
        .unwrap()
}
fn params() -> MessageCreateParams {
    MessageCreateParams::simple("hello", KnownModel::ClaudeHaiku55)
}

#[tokio::test]
async fn disconnected_connection_before_headers_should_retry() {
    let (url, attempts, task) = server(vec![], ok_response()).await;
    let result = client(url).send(params()).await;
    task.abort();
    assert!(
        result.is_ok(),
        "transient disconnect failed after {} attempt(s): {result:?}",
        attempts.load(Ordering::SeqCst)
    );
}
#[tokio::test]
async fn truncated_503_body_should_still_retry() {
    let partial =
        b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{"
            .to_vec();
    let (url, attempts, task) = server(partial, ok_response()).await;
    let result = client(url).send(params()).await;
    task.abort();
    assert!(
        result.is_ok(),
        "503 with disconnected error body failed after {} attempt(s): {result:?}",
        attempts.load(Ordering::SeqCst)
    );
}
#[tokio::test]
async fn request_id_should_be_preserved_for_server_error() {
    let body = r#"{"type":"error","error":{"type":"api_error","message":"failed"},"request_id":"req_body"}"#;
    let response = format!("HTTP/1.1 500 Internal Server Error\r\nrequest-id: req_header\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes();
    let (url, _, task) = server(response, vec![]).await;
    let err = client(url)
        .with_max_retries(0)
        .send(params())
        .await
        .unwrap_err();
    task.abort();
    assert_eq!(err.request_id(), Some("req_header"));
}

fn error_response(status: u16, headers: &str) -> Vec<u8> {
    let body = r#"{"error":{"type":"synthetic_error","message":"failed"},"request_id":"req_body"}"#;
    format!(
        "HTTP/1.1 {status} Error\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

#[tokio::test]
async fn retryable_statuses_retry_once() {
    for status in [408, 409, 429, 500, 501, 502, 503, 504, 529] {
        let (url, attempts, task) = server(
            error_response(status, "retry-after-ms: 1\r\n"),
            ok_response(),
        )
        .await;
        assert!(client(url).send(params()).await.is_ok(), "{status}");
        assert_eq!(attempts.load(Ordering::SeqCst), 2, "{status}");
        task.abort();
    }
}

#[tokio::test]
async fn http_metadata_survives_each_error_kind() {
    for status in [400, 401, 403, 404, 408, 409, 429, 500, 503, 529] {
        let (url, _, task) = server(error_response(status, "retry-after: 86400\r\n"), vec![]).await;
        let err = client(url).send(params()).await.unwrap_err();
        assert_eq!(err.status_code(), Some(status));
        assert_eq!(err.request_id(), Some("req_body"));
        assert_eq!(err.retry_after(), Some(Duration::from_secs(86400)));
        assert_eq!(
            err.metadata().unwrap().error_type.as_deref(),
            Some("synthetic_error")
        );
        if status == 529 {
            assert!(matches!(err, Error::Overloaded { .. }));
        }
        task.abort();
    }
}

#[tokio::test]
async fn http_date_retry_after_is_preserved_without_sleep() {
    let (url, attempts, task) = server(
        error_response(503, "retry-after: Wed, 21 Oct 2099 07:28:00 GMT\r\n"),
        ok_response(),
    )
    .await;
    let err = client(url).send(params()).await.unwrap_err();
    assert!(err.retry_after().unwrap() > Duration::from_secs(60));
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    task.abort();
}

#[tokio::test]
async fn truncated_200_and_builder_errors_are_not_retried() {
    let (url, attempts, task) = server(b"HTTP/1.1 200 OK\r\nrequest-id: req_partial\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{".to_vec(), ok_response()).await;
    let err = client(url).send(params()).await.unwrap_err();
    assert!(!err.is_retryable());
    assert_eq!(err.request_id(), Some("req_partial"));
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    task.abort();
    let err = client("invalid://bad".into())
        .send(params())
        .await
        .unwrap_err();
    assert!(!err.is_retryable());
}

#[tokio::test]
async fn timeout_does_not_repeat_a_billed_post() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let attempts = Arc::new(AtomicUsize::new(0));
    let seen = attempts.clone();
    let task = tokio::spawn(async move {
        let mut sockets = Vec::new();
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            seen.fetch_add(1, Ordering::SeqCst);
            sockets.push(socket);
        }
    });
    let err = client(url)
        .with_max_retries(3)
        .with_timeout(Duration::from_millis(30))
        .unwrap()
        .send(params())
        .await
        .unwrap_err();
    assert!(err.is_timeout());
    assert!(!err.is_retryable());
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    task.abort();
}

#[tokio::test]
async fn invalid_200_retains_request_id_and_length() {
    let (url, attempts, task) = server(b"HTTP/1.1 200 OK\r\nrequest-id: req_decode\r\nContent-Length: 3\r\nConnection: close\r\n\r\nbad".to_vec(), ok_response()).await;
    let err = client(url).send(params()).await.unwrap_err();
    assert_eq!(err.request_id(), Some("req_decode"));
    assert!(err.to_string().contains("3 bytes"));
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    task.abort();
}

#[tokio::test]
async fn batch_timeout_is_not_retried() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let attempts = Arc::new(AtomicUsize::new(0));
    let seen = attempts.clone();
    let server = tokio::spawn(async move {
        let mut sockets = Vec::new();
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            seen.fetch_add(1, Ordering::SeqCst);
            sockets.push(socket);
        }
    });
    let batch =
        MessageBatchCreateParams::new(vec![MessageBatchCreateRequest::new("test", params())]);
    let err = client(url)
        .with_max_retries(3)
        .with_timeout(Duration::from_millis(30))
        .unwrap()
        .create_message_batch(batch)
        .await
        .unwrap_err();
    assert!(err.is_timeout());
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn connection_refused_retries_when_listener_becomes_available() {
    let reservation = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = reservation.local_addr().unwrap();
    drop(reservation);
    let task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let listener = TcpListener::bind(addr).await.unwrap();
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0; 32768];
        socket.read(&mut buf).await.unwrap();
        socket.write_all(&ok_response()).await.unwrap();
    });
    client(format!("http://{addr}"))
        .send(params())
        .await
        .unwrap();
    task.await.unwrap();
}
