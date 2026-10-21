use base64::Engine;
use serde_json::json;
use topcoat_si::*;

#[test]
fn web_fetch_error_round_trip_preserves_required_type() {
    let wire = json!({"type":"web_fetch_tool_result", "tool_use_id":"srvtoolu_123", "content":{"type":"web_fetch_tool_result_error", "error_code":"max_uses_exceeded"}});
    let block: ContentBlock = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(block).unwrap(), wire);
}

#[test]
fn cache_prewarm_allows_zero_output_tokens() {
    let params = MessageCreateParams::new(
        0,
        vec![MessageParam::new_with_blocks(
            vec![
                TextBlock::new("warmup")
                    .with_cache_control(CacheControlEphemeral::new())
                    .into(),
            ],
            MessageRole::User,
        )],
        KnownModel::ClaudeOpus55.into(),
    );
    assert!(params.validate().is_ok());
}

#[test]
fn accepts_base64_image_below_anthropic_limit() {
    // Valid 640x640 RGB PNG: 1,229,883 raw bytes / 1,639,844 base64 bytes.
    let data = base64::engine::general_purpose::STANDARD
        .encode(include_bytes!("../fixtures/review_valid_640px.png"));
    let source = Base64ImageSource::new(data, ImageMediaType::Png);
    let message = MessageParam::new_with_blocks(
        vec![ContentBlock::Image(ImageBlock::new_with_base64(source))],
        MessageRole::User,
    );
    let params = MessageCreateParams::new(100, vec![message], KnownModel::ClaudeOpus55.into());
    assert!(params.validate().is_ok());
}

#[test]
fn ordinary_usage_iteration_can_omit_model() {
    let wire = json!({"input_tokens":1,"output_tokens":1,"iterations":[{"type":"message","input_tokens":1,"output_tokens":1,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}]});
    assert!(serde_json::from_value::<Usage>(wire).is_ok());
}

#[test]
fn single_pdf_page_uses_exclusive_endpoint() {
    let citation = CitationPageLocation::new("one page".into(), 0, 1, 2, None);
    assert_eq!(citation.page_count(), 1);
    assert!(citation.is_single_page());
}

#[test]
fn content_document_serializes_one_type_key() {
    let document =
        DocumentBlock::new_with_content_block(ContentBlockSourceParam::from_string_ref("hello"));
    let json = serde_json::to_string(&document).unwrap();
    let parsed = serde_json::from_str::<DocumentBlock>(&json);
    assert!(parsed.is_ok(), "{json}: {parsed:?}");
}

#[test]
fn future_response_enums_do_not_discard_messages() {
    assert_eq!(
        serde_json::from_value::<StopReason>(json!("future_stop")).unwrap(),
        StopReason::Unknown
    );
    assert!(matches!(
        serde_json::from_value::<ContentBlockDelta>(json!({"type":"future_delta","value":1}))
            .unwrap(),
        ContentBlockDelta::Unknown
    ));
}

#[test]
fn all_usage_iteration_kinds_and_null_model_decode() {
    for kind in [
        "message",
        "fallback_message",
        "compaction",
        "advisor_message",
        "future_kind",
    ] {
        let iteration = json!({"type":kind,"model":null,"input_tokens":1,"output_tokens":2,"cache_creation_input_tokens":0,"cache_read_input_tokens":0});
        assert!(
            serde_json::from_value::<UsageIteration>(iteration).is_ok(),
            "{kind}"
        );
    }
}

#[test]
fn cache_prewarm_rejects_output_producing_options() {
    let base = MessageCreateParams::new(
        0,
        vec![MessageParam::user("warmup")],
        KnownModel::ClaudeOpus55.into(),
    );
    let mut params = base.clone();
    params.stream = true;
    assert!(params.validate().is_err());
    params = base.clone();
    params.thinking = Some(ThinkingConfig::Adaptive);
    assert!(params.validate().is_err());
    params = base.clone();
    params.tool_choice = Some(ToolChoice::any());
    assert!(params.validate().is_err());
    params = base.clone();
    params.tool_choice = Some(ToolChoice::tool("test"));
    assert!(params.validate().is_err());
    params = base;
    params.output_config = Some(
        serde_json::from_value(json!({"format":{"type":"json_schema","schema":{"type":"object"}}}))
            .unwrap(),
    );
    assert!(params.validate().is_err());
}
