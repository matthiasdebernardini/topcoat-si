//! Compile-time and no-network check of the API surface the topcoat app relies on.

use std::str::FromStr;

use claudius::{
    AccumulatingStream, Anthropic, ContentBlock, HttpMetadata, Message, MessageCreateParams,
    MessageParam, Model, StopReason, ToolUnionParam, WebSearchTool20250305,
};

/// Mirrors topcoat's streaming call. The future is built but never polled.
async fn call(client: &Anthropic, params: MessageCreateParams) -> Result<Message, claudius::Error> {
    let events = client.stream(&params).await?;
    let (mut acc, done) = AccumulatingStream::new(events);
    drop(done);
    let _ = &mut acc;
    Err(claudius::Error::streaming(
        "stream ended without a final message",
        None,
    ))
}

fn text_of(message: &Message) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn topcoat_surface_compiles_and_builds_without_network() {
    let client = Anthropic::new(Some("test-key".to_string()))
        .unwrap()
        .with_base_url("http://127.0.0.1:9".into())
        .unwrap()
        .with_max_retries(0);

    let known = Model::from_str("claude-sonnet-5-5").unwrap_or_else(|()| Model::Custom("x".into()));
    assert!(matches!(known, Model::Known(_)));
    let custom = Model::from_str("some-custom-model")
        .unwrap_or_else(|()| Model::Custom("some-custom-model".to_string()));
    assert!(matches!(custom, Model::Custom(ref name) if name == "some-custom-model"));

    let max_searches: i32 = 3;
    let search = WebSearchTool20250305::new().with_max_uses(max_searches);
    let params = MessageCreateParams::new_streaming(
        1024,
        vec![MessageParam::from("hello".to_string())],
        known,
    )
    .with_system("system prompt")
    .with_tools(vec![ToolUnionParam::WebSearch20250305(search)]);

    let _unpolled = call(&client, params);

    let err = claudius::Error::streaming("stream ended without a final message", None);
    assert!(err.to_string().contains("stream ended"));

    let stop = Some(StopReason::MaxTokens);
    assert!(stop == Some(StopReason::MaxTokens));
    let _ = text_of;
}

#[test]
fn api_error_exposes_http_metadata() {
    let metadata = HttpMetadata {
        status: 500,
        request_id: Some("req_123".to_string()),
        ..HttpMetadata::default()
    };
    let err = claudius::Error::from_http("internal error", metadata, None);
    assert!(err.metadata().is_some());
    let formatted = format!("{:?}", err.metadata());
    assert!(formatted.contains("req_123"));
    assert!(formatted.contains("500"));
}

#[test]
fn message_stop_reason_max_tokens_deserializes() {
    let message: Message = serde_json::from_str(
        r#"{"id":"msg_1","type":"message","role":"assistant","model":"claude-sonnet-5-5","content":[],"stop_reason":"max_tokens","stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":1}}"#,
    )
    .unwrap();
    assert_eq!(message.stop_reason, Some(StopReason::MaxTokens));
}
