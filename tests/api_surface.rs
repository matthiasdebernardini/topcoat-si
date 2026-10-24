//! Compile-time and no-network check of the API surface the topcoat app relies on.

use std::time::Duration;

use carpa::{
    AccumulatingStream, Anthropic, ContentBlock, Error, ErrorKind, ErrorType, FirecrawlClient,
    FirecrawlWebSearch, KnownModel, Message, MessageCreateParams, MessageParam, Model, Service,
    StopReason, ToolRunError,
};

/// Mirrors topcoat's streaming call. The future is built but never polled.
async fn call(
    client: &Anthropic,
    params: MessageCreateParams,
) -> Result<Message, carpa::Error> {
    let events = client.stream(&params).await?;
    let (mut acc, done) = AccumulatingStream::new(events);
    drop(done);
    let _ = &mut acc;
    Err(carpa::Error::tool(
        "stream ended without a final message",
        None,
    ))
}

fn text_of(message: &Message) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn topcoat_surface_compiles_and_builds_without_network() {
    use std::str::FromStr;

    let client = Anthropic::try_new(Some("test-key".to_string()))
        .unwrap()
        .with_base_url("http://127.0.0.1:9".into())
        .unwrap()
        .with_max_retries(0);

    let Ok(known) = Model::from_str("claude-sonnet-5-5");
    assert!(matches!(known, Model::Known(_)));
    let Ok(custom) = Model::from_str("some-custom-model");
    assert!(matches!(custom, Model::Custom(ref name) if name == "some-custom-model"));

    let search = FirecrawlWebSearch::new(
        FirecrawlClient::try_new("fc-test-key")
            .unwrap()
            .with_base_url("http://127.0.0.1:9")
            .unwrap(),
    );
    let params = MessageCreateParams::new_streaming(
        1024,
        vec![MessageParam::from("hello".to_string())],
        known,
    )
    .with_system("system prompt")
    .with_tools(vec![search.tool()]);

    let _unpolled = call(&client, params);
    let _ = text_of;
}

#[test]
fn api_error_exposes_its_status() {
    let err = carpa::Error::api(Service::Anthropic, Some(500), None, "internal error", None);
    assert!(matches!(err.kind(), ErrorKind::Api { .. }));
    assert_eq!(err.status(), Some(500));
    assert!(err.is_retryable());
    assert_eq!(
        err.to_string(),
        "Anthropic API error: internal error (HTTP 500)"
    );
}

#[test]
fn error_is_clone_send_sync_static() {
    fn assert_bounds<T: Clone + Send + Sync + 'static>() {}
    assert_bounds::<Error>();
    assert_bounds::<ErrorKind>();
    assert_bounds::<Service>();
}

/// A downstream crate must match with a wildcard, because `ErrorKind` is non-exhaustive.
fn describe(kind: &ErrorKind) -> String {
    match kind {
        ErrorKind::Api { service, .. } => service.to_string(),
        ErrorKind::Validation { .. } => "validation".to_string(),
        _ => "other".to_string(),
    }
}

#[test]
fn simulated_429_answers_the_three_questions() {
    let err = Error::api(
        Service::Firecrawl,
        Some(429),
        Some(ErrorType::RateLimitError),
        "slow down",
        Some(Duration::from_secs(7)),
    );
    // What failed and which service.
    assert_eq!(describe(err.kind()), "Firecrawl");
    assert!(matches!(
        err.kind(),
        ErrorKind::Api {
            service: Service::Firecrawl,
            status: Some(429),
            ..
        }
    ));
    // Whether sending again might work.
    assert!(err.is_retryable());
    assert_eq!(err.status(), Some(429));
    assert_eq!(err.retry_after(), Some(Duration::from_secs(7)));
    assert_eq!(err.request_id(), None);
    assert_eq!(err.events_delivered(), None);
    assert_eq!(
        err.to_string(),
        "Firecrawl API error: slow down (HTTP 429 rate_limit_error)"
    );
}

#[test]
fn stream_event_errors_have_no_status() {
    let overloaded = Error::api(
        Service::Anthropic,
        None,
        Some(ErrorType::OverloadedError),
        "Overloaded",
        None,
    );
    assert_eq!(overloaded.status(), None);
    assert!(overloaded.is_retryable());
    assert_eq!(
        overloaded.to_string(),
        "Anthropic API error: Overloaded (in stream, overloaded_error)"
    );

    let invalid = Error::api(
        Service::Anthropic,
        None,
        Some(ErrorType::InvalidRequestError),
        "bad",
        None,
    );
    assert!(!invalid.is_retryable());

    let unauthorized = Error::api(Service::Jev, Some(401), None, "no key", None);
    assert!(!unauthorized.is_retryable());
    assert_eq!(unauthorized.retry_after(), None);
}

#[test]
fn validation_and_tool_constructors_are_not_retryable() {
    let validation = Error::validation("max_tokens must be positive", Some("max_tokens".into()));
    assert_eq!(describe(validation.kind()), "validation");
    assert!(matches!(
        validation.kind(),
        ErrorKind::Validation { param: Some(param), .. } if param == "max_tokens"
    ));
    assert!(!validation.is_retryable());
    assert_eq!(validation.status(), None);
    assert_eq!(validation.retry_after(), None);

    let cause = std::io::Error::other("disk full");
    let tool = Error::tool("could not save the note", Some(Box::new(cause)));
    assert!(matches!(tool.kind(), ErrorKind::Tool));
    assert!(!tool.is_retryable());
    assert_eq!(tool.to_string(), "could not save the note");
    // The cause is the source, never part of the message.
    let source = std::error::Error::source(&tool).expect("tool error keeps its source");
    assert_eq!(source.to_string(), "disk full");

    assert!(std::error::Error::source(&Error::tool("no cause", None)).is_none());
}

#[tokio::test]
async fn tool_run_error_exposes_the_partial_transcript() {
    // `run_tools` fails before sending when `max_turns` is 0, with the caller's messages intact.
    let client = Anthropic::try_new(Some("test-key".to_string()))
        .unwrap()
        .with_base_url("http://127.0.0.1:9".into())
        .unwrap();
    let params = MessageCreateParams::new(
        16,
        vec![MessageParam::user("hello")],
        Model::Known(KnownModel::ClaudeHaiku55),
    );
    let failed: Box<ToolRunError> = client
        .run_tools(params, &[], 0)
        .await
        .map(|_| ())
        .unwrap_err();
    assert!(matches!(
        failed.error.kind(),
        ErrorKind::Validation { param: Some(param), .. } if param == "max_turns"
    ));
    assert!(failed.responses.is_empty());
    assert_eq!(failed.messages.len(), 1);
    assert!(std::error::Error::source(&*failed).is_some());
    assert_eq!(
        failed.to_string(),
        "tool run failed after 0 model responses"
    );
    assert!(failed.error.to_string().contains("max_turns"));
}

#[test]
fn message_stop_reason_max_tokens_deserializes() {
    let message: Message = serde_json::from_str(
        r#"{"id":"msg_1","type":"message","role":"assistant","model":"claude-sonnet-5-5","content":[],"stop_reason":"max_tokens","stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":1}}"#,
    )
    .unwrap();
    assert_eq!(message.stop_reason, Some(StopReason::MaxTokens));
}
