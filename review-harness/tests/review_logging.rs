use claudius::*;
use futures::StreamExt;
use std::sync::Mutex;
use serde_json::json;

#[derive(Default)]
struct Logger {
    completed: Mutex<Vec<Message>>,
    events: Mutex<usize>,
}
impl ClientLogger for Logger {
    fn log_response(&self, _: &Message) {}
    fn log_stream_event(&self, _: &MessageStreamEvent) { *self.events.lock().unwrap() += 1; }
    fn log_stream_message(&self, message: &Message) { self.completed.lock().unwrap().push(message.clone()); }
}

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
    Anthropic::new(Some("test".into())).unwrap().with_base_url(format!("http://{addr}"))
}

#[tokio::test]
async fn logging_stream_swallows_missing_message_error() {
    let client = serve(String::new()).await;
    let params = ().create_request(100,vec![MessageParam::user("hello")],true).await;
    let logger = Logger::default();
    let events: Vec<_> = client.stream_with_logger(&params,&logger).await.unwrap().collect().await;
    assert!(events.is_empty(),"Empty server response becomes clean EOF, with no Err item");
    assert!(logger.completed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn failed_stream_is_logged_as_successfully_completed() {
    let message = Message::new("msg_partial".into(), vec![], KnownModel::ClaudeSonnet40.into(),Usage::new(100,0));
    let body = format!("event: message_start\ndata: {}\n\nevent: error\ndata: {}\n\n",json!({"type":"message_start","message":message}),json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}));
    let client = serve(body).await;
    let params = ().create_request(100,vec![MessageParam::user("hello")],true).await;
    let logger = Logger::default();
    let events: Vec<_> = client.stream_with_logger(&params,&logger).await.unwrap().collect().await;
    assert_eq!(events.iter().filter(|event| event.is_err()).count(),1);
    assert_eq!(logger.completed.lock().unwrap().len(),1,"Failure incorrectly triggers success completion callback");
    assert_eq!(logger.completed.lock().unwrap()[0].stop_reason,None);
}
