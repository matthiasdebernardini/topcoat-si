use claudius::{Anthropic, KnownModel, MessageCreateParams};
use std::error::Error;
use std::time::Duration;

#[test]
fn client_debug_must_redact_credentials() {
    let sentinel = "sk-ant-review-dummy-secret-NOT-A-REAL-KEY";
    let client = Anthropic::new(Some(sentinel.into())).unwrap();
    let dump = format!("{client:?}");
    assert!(
        !dump.contains(sentinel),
        "client Debug contains dummy API credential {} times",
        dump.matches(sentinel).count()
    );
}

#[test]
fn baseten_client_debug_must_redact_credentials() {
    let sentinel = "sk-review-dummy-secret-NOT-A-REAL-KEY";
    let client = Anthropic::new(Some(sentinel.into()))
        .unwrap()
        .with_base_url("https://inference.baseten.co".into())
        .unwrap();
    let dump = format!("{client:?}");
    assert!(
        !dump.contains(sentinel),
        "client Debug contains dummy API credential {} times",
        dump.matches(sentinel).count()
    );
}

#[tokio::test]
async fn network_timeout_preserves_source_chain() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
    });
    let client = Anthropic::new(Some("dummy-key".into()))
        .unwrap()
        .with_base_url(format!("http://{address}"))
        .unwrap()
        .with_max_retries(0)
        .with_timeout(Duration::from_millis(50))
        .unwrap();
    let err = client
        .send(MessageCreateParams::simple(
            "hello",
            KnownModel::ClaudeHaiku55,
        ))
        .await
        .unwrap_err();
    task.abort();
    assert!(err.is_timeout(), "{err:?}");
    assert!(
        err.source().is_some(),
        "timeout discarded its reqwest source: {err}"
    );
}

#[test]
fn auth_header_debug_redacts_both_credentials() {
    let client = Anthropic::new(Some("  synthetic-secret  ".into()))
        .unwrap()
        .with_base_url("https://inference.baseten.co".into())
        .unwrap();
    assert_eq!(client.api_key(), "synthetic-secret");
    let headers = client.default_headers();
    assert!(!format!("{headers:?}").contains("synthetic-secret"));
    assert!(headers["x-api-key"].is_sensitive());
    assert!(headers["authorization"].is_sensitive());
}

#[tokio::test]
async fn redirect_does_not_forward_api_key() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let destination = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = origin.local_addr().unwrap();
    let location = format!("http://{}/leak", destination.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = origin.accept().await.unwrap();
        let mut buf = [0; 16384];
        socket.read(&mut buf).await.unwrap();
        socket.write_all(format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n").as_bytes()).await.unwrap();
    });
    let client = Anthropic::new(Some("synthetic-secret".into()))
        .unwrap()
        .with_base_url(format!("http://{address}"))
        .unwrap();
    assert!(
        client
            .send(MessageCreateParams::simple(
                "hello",
                KnownModel::ClaudeHaiku55
            ))
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), destination.accept())
            .await
            .is_err()
    );
    task.await.unwrap();
}

#[test]
fn empty_primary_env_key_falls_back_and_trims() {
    // nextest runs this test in its own process; set env before constructing a runtime.
    unsafe {
        std::env::set_var("CLAUDIUS_API_KEY", "  ");
        std::env::set_var("ANTHROPIC_API_KEY", "  synthetic-fallback  ");
    }
    assert_eq!(
        Anthropic::new(None).unwrap().api_key(),
        "synthetic-fallback"
    );
}
