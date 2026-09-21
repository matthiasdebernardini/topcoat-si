use claudius::*;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::ops::ControlFlow;
use serde_json::json;

struct AdapterAgent { called: AtomicBool }
#[async_trait::async_trait]
impl Agent for AdapterAgent {
    async fn tools(&self) -> Vec<Arc<dyn Tool<Self>>> { vec![Arc::new(ToolBash20250124::new())] }
    async fn bash(&self, _: &str, restart: bool) -> std::result::Result<String, std::io::Error> {
        self.called.store(restart, Ordering::Relaxed);
        Ok("restarted".into())
    }
    async fn view(&self, _: &str, _: Option<(u32,u32)>) -> std::result::Result<String, std::io::Error> {
        self.called.store(true, Ordering::Relaxed);
        Ok("file content".into())
    }
}

#[tokio::test]
async fn documented_restart_is_rejected_before_callback() {
    let mut agent = AdapterAgent { called: AtomicBool::new(false) };
    let client = Anthropic::new(Some("test".into())).unwrap();
    let response = Message::new("m".into(), vec![ToolUseBlock::new("t", "bash", json!({"restart":true})).into()], KnownModel::ClaudeSonnet40.into(), Usage::new(1,1));
    let ControlFlow::Continue(results) = agent.handle_default_tool_use(&client, &response).await else { panic!() };
    assert!(!agent.called.load(Ordering::Relaxed));
    assert!(serde_json::to_string(&results).unwrap().contains("missing field `command`"));
}

#[tokio::test]
async fn documented_end_of_file_range_is_rejected_before_callback() {
    let agent = AdapterAgent { called: AtomicBool::new(false) };
    let result = agent.text_editor(ToolUseBlock::new("t", "str_replace_based_edit_tool", json!({"command":"view","path":"test","view_range":[2,-1]}))).await;
    assert!(result.unwrap_err().to_string().contains("expected u32"));
    assert!(!agent.called.load(Ordering::Relaxed));
}

async fn serve(response: Message) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 32768];
        let _ = socket.read(&mut buf).await.unwrap();
        let body = serde_json::to_string(&response).unwrap();
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).as_bytes()).await.unwrap();
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn over_budget_completed_answer_disappears() {
    let response = Message::new("m".into(), vec![TextBlock::new("The completed answer").into()], KnownModel::ClaudeSonnet40.into(), Usage::new(101,1)).with_stop_reason(StopReason::EndTurn);
    let client = Anthropic::new(Some("test".into())).unwrap().with_base_url(serve(response).await);
    let budget = Arc::new(Budget::new_flat_rate(100,1));
    let mut agent = ();
    let mut messages = vec![MessageParam::user("hello")];
    let outcome = agent.take_turn(&client, &mut messages, &budget).await.unwrap();
    assert_eq!(outcome.request_count,1);
    assert_eq!(outcome.stop_reason, StopReason::MaxTokens);
    assert_eq!(messages.len(),1, "Completed answer was discarded");
}

struct PausingAgent { max_seen: std::sync::Mutex<Vec<u32>> }
#[async_trait::async_trait]
impl Agent for PausingAgent {
    async fn max_tokens(&self) -> u32 { 10 }
    async fn hook_message_create_params(&self, req: &MessageCreateParams) -> std::result::Result<(), Error> {
        self.max_seen.lock().unwrap().push(req.max_tokens);
        if req.max_tokens == 0 { return Err(Error::unknown("attempted zero token request")); }
        Ok(())
    }
}

#[tokio::test]
async fn pause_turn_submits_zero_token_request_after_budget_exhaustion() {
    let response = Message::new("m".into(), vec![TextBlock::new("paused").into()], KnownModel::ClaudeSonnet40.into(), Usage::new(95,5)).with_stop_reason(StopReason::PauseTurn);
    let client = Anthropic::new(Some("test".into())).unwrap().with_base_url(serve(response).await);
    let budget = Arc::new(Budget::new_flat_rate(100,1));
    let mut agent = PausingAgent { max_seen: std::sync::Mutex::new(vec![]) };
    let mut messages = vec![MessageParam::user("hello")];
    let err = agent.take_turn(&client, &mut messages, &budget).await.unwrap_err();
    assert!(err.to_string().contains("attempted zero token request"));
    assert_eq!(*agent.max_seen.lock().unwrap(), vec![10,0]);
}
