use claudius::*;
use base64::Engine;
use serde_json::json;

#[test]
fn web_search_error_round_trip_preserves_required_type() {
    let wire = json!({"type":"web_search_tool_result", "tool_use_id":"srvtoolu_123", "content":{"type":"web_search_tool_result_error", "error_code":"max_uses_exceeded"}});
    let block: ContentBlock = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(block).unwrap(), wire);
}

#[test]
fn web_search_request_too_large_is_valid_response() {
    let wire = json!({"type":"web_search_tool_result", "tool_use_id":"srvtoolu_123", "content":{"type":"web_search_tool_result_error", "error_code":"request_too_large"}});
    assert!(serde_json::from_value::<ContentBlock>(wire).is_ok());
}

#[test]
fn cache_prewarm_allows_zero_output_tokens() {
    let params = MessageCreateParams::new(0, vec![MessageParam::user("warmup")], KnownModel::ClaudeOpus5.into());
    assert!(params.validate().is_ok());
}

#[test]
fn accepts_base64_image_below_anthropic_limit() {
    // Valid 640x640 RGB PNG: 1,229,883 raw bytes / 1,639,844 base64 bytes.
    let data = base64::engine::general_purpose::STANDARD.encode(include_bytes!("fixtures/review_valid_640px.png"));
    let source = Base64ImageSource::new(data, ImageMediaType::Png);
    let message = MessageParam::new_with_blocks(vec![ContentBlock::Image(ImageBlock::new_with_base64(source))], MessageRole::User);
    let params = MessageCreateParams::new(100, vec![message], KnownModel::ClaudeOpus5.into());
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
    let document = DocumentBlock::new_with_content_block(ContentBlockSourceParam::from_string_ref("hello"));
    let json = serde_json::to_string(&document).unwrap();
    let parsed = serde_json::from_str::<DocumentBlock>(&json);
    assert!(parsed.is_ok(), "{json}: {parsed:?}");
}
