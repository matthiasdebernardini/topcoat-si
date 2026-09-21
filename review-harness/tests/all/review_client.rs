use claudius::*;
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use std::time::Duration;

async fn server(first: Vec<u8>, second: Vec<u8>) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
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
                if n == 0 { break; }
                request.extend_from_slice(&buf[..n]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                    let len: usize = headers.lines().find_map(|l| l.strip_prefix("content-length:").map(|s| s.trim().parse().unwrap())).unwrap_or(0);
                    if request.len() >= end + 4 + len { break; }
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
    Anthropic::new(Some("dummy-key".into())).unwrap().with_base_url(url).with_max_retries(1).with_backoff_params(1000.0, 1000.0)
        .with_timeout(Duration::from_secs(2)).unwrap()
}
fn params() -> MessageCreateParams { MessageCreateParams::simple("hello", KnownModel::ClaudeHaiku45) }

#[tokio::test]
async fn disconnected_connection_before_headers_should_retry() {
    let (url, attempts, task) = server(vec![], ok_response()).await;
    let result = client(url).send(params()).await;
    task.abort();
    assert!(result.is_ok(), "transient disconnect failed after {} attempt(s): {result:?}", attempts.load(Ordering::SeqCst));
}
#[tokio::test]
async fn truncated_503_body_should_still_retry() {
    let partial = b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{".to_vec();
    let (url, attempts, task) = server(partial, ok_response()).await;
    let result = client(url).send(params()).await;
    task.abort();
    assert!(result.is_ok(), "503 with disconnected error body failed after {} attempt(s): {result:?}", attempts.load(Ordering::SeqCst));
}
#[tokio::test]
async fn request_id_should_be_preserved_for_server_error() {
    let body = r#"{"type":"error","error":{"type":"api_error","message":"failed"},"request_id":"req_body"}"#;
    let response = format!("HTTP/1.1 500 Internal Server Error\r\nrequest-id: req_header\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes();
    let (url, _, task) = server(response, vec![]).await;
    let err = client(url).with_max_retries(0).send(params()).await.unwrap_err();
    task.abort();
    assert_eq!(err.request_id(), Some("req_header"));
}
