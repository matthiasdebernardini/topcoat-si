use claudius::{Anthropic, KnownModel, MessageCreateParams};
use std::error::Error;
use std::time::Duration;

#[test]
fn client_debug_must_redact_credentials() {
    let sentinel = "sk-ant-review-dummy-secret-NOT-A-REAL-KEY";
    let client = Anthropic::new(Some(sentinel.into())).unwrap();
    let dump = format!("{client:?}");
    assert!(!dump.contains(sentinel), "client Debug contains dummy API credential {} times", dump.matches(sentinel).count());
}

#[test]
fn baseten_client_debug_must_redact_credentials() {
    let sentinel = "sk-review-dummy-secret-NOT-A-REAL-KEY";
    let client = Anthropic::new(Some(sentinel.into())).unwrap()
        .with_base_url("https://inference.baseten.co".into());
    let dump = format!("{client:?}");
    assert!(!dump.contains(sentinel), "client Debug contains dummy API credential {} times", dump.matches(sentinel).count());
}

#[tokio::test]
async fn network_timeout_preserves_source_chain() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
    });
    let client = Anthropic::new(Some("dummy-key".into())).unwrap()
        .with_base_url(format!("http://{address}"))
        .with_max_retries(0)
        .with_timeout(Duration::from_millis(50)).unwrap();
    let err = client.send(MessageCreateParams::simple("hello", KnownModel::ClaudeHaiku45)).await.unwrap_err();
    task.abort();
    assert!(err.is_timeout(), "{err:?}");
    assert!(err.source().is_some(), "timeout discarded its reqwest source: {err}");
}
