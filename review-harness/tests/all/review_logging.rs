use claudius::*;
use futures::StreamExt;
use serde_json::json;

async fn serve(body: String) -> Anthropic {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 32768];
        socket.read(&mut buf).await.unwrap();
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).as_bytes()).await.unwrap();
    });
    Anthropic::new(Some("test".into()))
        .unwrap()
        .with_base_url(format!("http://{addr}"))
        .unwrap()
}

#[tokio::test]
async fn public_stream_reports_missing_message_error() {
    let client = serve(String::new()).await;
    let params = MessageCreateParams::new_streaming(
        100,
        vec![MessageParam::user("hello")],
        KnownModel::ClaudeSonnet55.into(),
    );
    let events: Vec<_> = client.stream(&params).await.unwrap().collect().await;
    assert_eq!(events.len(), 1);
    assert!(
        events[0]
            .as_ref()
            .unwrap_err()
            .to_string()
            .contains("message")
    );
}

#[tokio::test]
async fn public_stream_reports_failure_once() {
    let message = Message::new(
        "msg_partial".into(),
        vec![],
        KnownModel::ClaudeSonnet55.into(),
        Usage::new(100, 0),
    );
    let body = format!(
        "event: message_start\ndata: {}\n\nevent: error\ndata: {}\n\n",
        json!({"type":"message_start","message":message}),
        json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}})
    );
    let client = serve(body).await;
    let params = MessageCreateParams::new_streaming(
        100,
        vec![MessageParam::user("hello")],
        KnownModel::ClaudeSonnet55.into(),
    );
    let events: Vec<_> = client.stream(&params).await.unwrap().collect().await;
    assert_eq!(events.iter().filter(|event| event.is_err()).count(), 1);
}
