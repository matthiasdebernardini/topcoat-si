use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::Value;

use crate::types::open_union;
use crate::types::{
    CodeExecutionToolResultBlock, CompactionBlock, DocumentBlock, FallbackBlock, ImageBlock,
    McpToolResultBlock, McpToolUseBlock, RedactedThinkingBlock, SearchResultBlock,
    ServerToolUseBlock, TextBlock, ThinkingBlock, ToolReference, ToolResultBlock,
    ToolSearchToolResultBlock, ToolUseBlock, WebFetchToolResultBlock,
};

/// A block of content in a message.
///
/// This enum represents the different types of content blocks that can be included
/// in a message's content. A block whose `type` is not recognized is kept verbatim in
/// [`ContentBlock::Unknown`] and serialized back unchanged, so it survives being
/// echoed back in the conversation history.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum ContentBlock {
    /// A block of text content
    #[serde(rename = "text")]
    Text(TextBlock),

    /// An image block
    #[serde(rename = "image")]
    Image(ImageBlock),

    /// A block representing a tool use request
    #[serde(rename = "tool_use")]
    ToolUse(ToolUseBlock),

    /// A block representing a server-side tool use request
    #[serde(rename = "server_tool_use")]
    ServerToolUse(ServerToolUseBlock),

    /// A tool result block
    #[serde(rename = "tool_result")]
    ToolResult(ToolResultBlock),

    /// A document block
    #[serde(rename = "document")]
    Document(DocumentBlock),

    /// A block containing model thinking
    #[serde(rename = "thinking")]
    Thinking(ThinkingBlock),

    /// A block containing redacted thinking data
    #[serde(rename = "redacted_thinking")]
    RedactedThinking(RedactedThinkingBlock),

    /// A boundary between a refusing model and the model that continued generation.
    #[serde(rename = "fallback")]
    Fallback(FallbackBlock),

    /// A search result supplied as input, enabling RAG-style citations
    #[serde(rename = "search_result")]
    SearchResult(SearchResultBlock),

    /// The result of a server-side web fetch
    #[serde(rename = "web_fetch_tool_result")]
    WebFetchToolResult(WebFetchToolResultBlock),

    /// The result of a server-side code execution
    #[serde(rename = "code_execution_tool_result")]
    CodeExecutionToolResult(CodeExecutionToolResultBlock),

    /// The result of a server-side bash code execution (content kept opaque)
    #[serde(rename = "bash_code_execution_tool_result")]
    BashCodeExecutionToolResult {
        /// The ID of the `server_tool_use` block this result answers.
        tool_use_id: String,
        /// The result payload, kept verbatim.
        content: Value,
    },

    /// The result of a server-side text editor code execution (content kept opaque)
    #[serde(rename = "text_editor_code_execution_tool_result")]
    TextEditorCodeExecutionToolResult {
        /// The ID of the `server_tool_use` block this result answers.
        tool_use_id: String,
        /// The result payload, kept verbatim.
        content: Value,
    },

    /// The result of a server-side tool search
    #[serde(rename = "tool_search_tool_result")]
    ToolSearchToolResult(ToolSearchToolResultBlock),

    /// A file made available to the code execution container
    #[serde(rename = "container_upload")]
    ContainerUpload {
        /// The ID of the uploaded file.
        file_id: String,
    },

    /// A summary of earlier conversation produced by compaction
    #[serde(rename = "compaction")]
    Compaction(CompactionBlock),

    /// A tool call made against a remote MCP server
    #[serde(rename = "mcp_tool_use")]
    McpToolUse(McpToolUseBlock),

    /// The result of an MCP tool call
    #[serde(rename = "mcp_tool_result")]
    McpToolResult(McpToolResultBlock),

    /// A reference to a tool discovered by tool search
    #[serde(rename = "tool_reference")]
    ToolReference(ToolReference),

    /// A block of a type this crate does not know, kept verbatim.
    #[serde(untagged)]
    Unknown(Value),
}

impl<'de> Deserialize<'de> for ContentBlock {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let block_type = value
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| de::Error::missing_field("type"))?
            .to_string();

        match block_type.as_str() {
            "text" => open_union::from_value(value).map(ContentBlock::Text),
            "image" => open_union::from_value(value).map(ContentBlock::Image),
            "tool_use" => open_union::from_value(value).map(ContentBlock::ToolUse),
            "server_tool_use" => open_union::from_value(value).map(ContentBlock::ServerToolUse),
            "tool_result" => open_union::from_value(value).map(ContentBlock::ToolResult),
            "document" => open_union::from_value(value).map(ContentBlock::Document),
            "thinking" | "thinking_summary" | "summary" => {
                open_union::from_value(value).map(ContentBlock::Thinking)
            }
            "redacted_thinking" => {
                open_union::from_value(value).map(ContentBlock::RedactedThinking)
            }
            "fallback" => open_union::from_value(value).map(ContentBlock::Fallback),
            "search_result" => open_union::from_value(value).map(ContentBlock::SearchResult),
            "web_fetch_tool_result" => {
                open_union::from_value(value).map(ContentBlock::WebFetchToolResult)
            }
            "code_execution_tool_result" => {
                open_union::from_value(value).map(ContentBlock::CodeExecutionToolResult)
            }
            "bash_code_execution_tool_result" => {
                let (tool_use_id, content) = opaque_result::<D::Error>(value)?;
                Ok(ContentBlock::BashCodeExecutionToolResult {
                    tool_use_id,
                    content,
                })
            }
            "text_editor_code_execution_tool_result" => {
                let (tool_use_id, content) = opaque_result::<D::Error>(value)?;
                Ok(ContentBlock::TextEditorCodeExecutionToolResult {
                    tool_use_id,
                    content,
                })
            }
            "tool_search_tool_result" => {
                open_union::from_value(value).map(ContentBlock::ToolSearchToolResult)
            }
            "container_upload" => {
                #[derive(Deserialize)]
                struct Upload {
                    file_id: String,
                }
                open_union::from_value::<Upload, D::Error>(value).map(|upload| {
                    ContentBlock::ContainerUpload {
                        file_id: upload.file_id,
                    }
                })
            }
            "compaction" => open_union::from_value(value).map(ContentBlock::Compaction),
            "mcp_tool_use" => open_union::from_value(value).map(ContentBlock::McpToolUse),
            "mcp_tool_result" => open_union::from_value(value).map(ContentBlock::McpToolResult),
            "tool_reference" => open_union::from_value(value).map(ContentBlock::ToolReference),
            tag => {
                open_union::warn_unknown("ContentBlock", tag);
                Ok(ContentBlock::Unknown(value))
            }
        }
    }
}

/// Reads the fields of a block whose `content` is kept opaque.
fn opaque_result<E: de::Error>(value: Value) -> Result<(String, Value), E> {
    #[derive(Deserialize)]
    struct Opaque {
        tool_use_id: String,
        content: Value,
    }
    open_union::from_value::<Opaque, E>(value).map(|opaque| (opaque.tool_use_id, opaque.content))
}

impl ContentBlock {
    /// Returns the raw JSON if this is an unrecognized block, or None otherwise.
    #[must_use]
    pub fn as_unknown(&self) -> Option<&Value> {
        match self {
            ContentBlock::Unknown(value) => Some(value),
            _ => None,
        }
    }

    /// Returns the wire `type` of this block.
    ///
    /// For an unrecognized block this is the stored `type` string, or `"unknown"` if
    /// the stored value has none.
    pub fn type_name(&self) -> &str {
        match self {
            ContentBlock::Text(_) => "text",
            ContentBlock::Image(_) => "image",
            ContentBlock::ToolUse(_) => "tool_use",
            ContentBlock::ServerToolUse(_) => "server_tool_use",
            ContentBlock::ToolResult(_) => "tool_result",
            ContentBlock::Document(_) => "document",
            ContentBlock::Thinking(_) => "thinking",
            ContentBlock::RedactedThinking(_) => "redacted_thinking",
            ContentBlock::Fallback(_) => "fallback",
            ContentBlock::SearchResult(_) => "search_result",
            ContentBlock::WebFetchToolResult(_) => "web_fetch_tool_result",
            ContentBlock::CodeExecutionToolResult(_) => "code_execution_tool_result",
            ContentBlock::BashCodeExecutionToolResult { .. } => "bash_code_execution_tool_result",
            ContentBlock::TextEditorCodeExecutionToolResult { .. } => {
                "text_editor_code_execution_tool_result"
            }
            ContentBlock::ToolSearchToolResult(_) => "tool_search_tool_result",
            ContentBlock::ContainerUpload { .. } => "container_upload",
            ContentBlock::Compaction(_) => "compaction",
            ContentBlock::McpToolUse(_) => "mcp_tool_use",
            ContentBlock::McpToolResult(_) => "mcp_tool_result",
            ContentBlock::ToolReference(_) => "tool_reference",
            ContentBlock::Unknown(value) => value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
        }
    }

    /// Returns a short bracketed label for display, such as `[web_fetch_tool_result]`.
    ///
    /// Unrecognized blocks are labelled `[unknown block: <type>]`.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            ContentBlock::Unknown(_) => format!("[unknown block: {}]", self.type_name()),
            other => format!("[{}]", other.type_name()),
        }
    }

    /// Returns a reference to the inner TextBlock if this is a Text variant,
    /// or None otherwise.
    #[must_use]
    pub fn as_text(&self) -> Option<&TextBlock> {
        match self {
            ContentBlock::Text(block) => Some(block),
            _ => None,
        }
    }

    /// Returns a reference to the inner ToolUseBlock if this is a ToolUse variant,
    /// or None otherwise.
    #[must_use]
    pub fn as_tool_use(&self) -> Option<&ToolUseBlock> {
        match self {
            ContentBlock::ToolUse(block) => Some(block),
            _ => None,
        }
    }
}

impl From<TextBlock> for ContentBlock {
    fn from(block: TextBlock) -> Self {
        ContentBlock::Text(block)
    }
}

impl From<ImageBlock> for ContentBlock {
    fn from(block: ImageBlock) -> Self {
        ContentBlock::Image(block)
    }
}

impl From<ToolUseBlock> for ContentBlock {
    fn from(block: ToolUseBlock) -> Self {
        ContentBlock::ToolUse(block)
    }
}

impl From<ServerToolUseBlock> for ContentBlock {
    fn from(block: ServerToolUseBlock) -> Self {
        ContentBlock::ServerToolUse(block)
    }
}

impl From<ToolResultBlock> for ContentBlock {
    fn from(block: ToolResultBlock) -> Self {
        ContentBlock::ToolResult(block)
    }
}

impl From<DocumentBlock> for ContentBlock {
    fn from(block: DocumentBlock) -> Self {
        ContentBlock::Document(block)
    }
}

impl From<ThinkingBlock> for ContentBlock {
    fn from(block: ThinkingBlock) -> Self {
        ContentBlock::Thinking(block)
    }
}

impl From<RedactedThinkingBlock> for ContentBlock {
    fn from(block: RedactedThinkingBlock) -> Self {
        ContentBlock::RedactedThinking(block)
    }
}

impl From<FallbackBlock> for ContentBlock {
    fn from(block: FallbackBlock) -> Self {
        ContentBlock::Fallback(block)
    }
}

impl From<SearchResultBlock> for ContentBlock {
    fn from(block: SearchResultBlock) -> Self {
        ContentBlock::SearchResult(block)
    }
}

impl From<CompactionBlock> for ContentBlock {
    fn from(block: CompactionBlock) -> Self {
        ContentBlock::Compaction(block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ImageSource, KnownModel, Model, UrlImageSource};

    #[test]
    fn text_block_serialization() {
        let text_block = TextBlock::new("This is some text content.");
        let content_block = ContentBlock::from(text_block);

        let json = serde_json::to_string(&content_block).unwrap();
        let expected = r#"{"type":"text","text":"This is some text content."}"#;

        assert_eq!(json, expected);
    }

    #[test]
    fn tool_use_block_serialization() {
        let input_json = serde_json::json!({
            "query": "weather in San Francisco",
            "limit": 5
        });

        let tool_block = ToolUseBlock::new("tool_123", "search", input_json);
        let content_block = ContentBlock::from(tool_block);

        let json = serde_json::to_string(&content_block).unwrap();
        let actual: serde_json::Value = serde_json::from_str(&json).unwrap();
        let expected: serde_json::Value = serde_json::json!({
            "type": "tool_use",
            "id": "tool_123",
            "input": {"limit": 5, "query": "weather in San Francisco"},
            "name": "search"
        });

        assert_eq!(actual, expected);
    }

    #[test]
    fn server_tool_use_block_serialization() {
        let server_block = ServerToolUseBlock::new(
            "tool_123",
            "web_fetch",
            serde_json::json!({"url": "https://example.com"}),
        );
        let content_block = ContentBlock::from(server_block);

        let json = serde_json::to_string(&content_block).unwrap();
        let expected = r#"{"type":"server_tool_use","id":"tool_123","input":{"url":"https://example.com"},"name":"web_fetch"}"#;

        assert_eq!(json, expected);
    }

    #[test]
    fn thinking_block_serialization() {
        let thinking_block = ThinkingBlock::new(
            "Let me think through this problem step by step...",
            "abc123signature",
        );
        let content_block = ContentBlock::from(thinking_block);

        let json = serde_json::to_string(&content_block).unwrap();
        let expected = r#"{"type":"thinking","signature":"abc123signature","thinking":"Let me think through this problem step by step..."}"#;

        assert_eq!(json, expected);
    }

    #[test]
    fn fallback_block_deserialization() {
        let json = serde_json::json!({
            "type": "fallback",
            "from": { "model": "claude-fable-5-1" },
            "to": { "model": "claude-opus-5-5" }
        });
        let block: ContentBlock = serde_json::from_value(json).unwrap();
        let ContentBlock::Fallback(block) = block else {
            panic!("expected fallback block");
        };
        assert_eq!(block.from.model, Model::Known(KnownModel::ClaudeFable51));
        assert_eq!(block.to.model, Model::Known(KnownModel::ClaudeOpus55));
    }

    #[test]
    fn thinking_summary_block_deserializes_as_thinking() {
        let json = r#"{"type":"thinking_summary","summary":"Condensed reasoning."}"#;
        let content_block: ContentBlock = serde_json::from_str(json).unwrap();

        let ContentBlock::Thinking(thinking) = content_block else {
            panic!("summary block should deserialize as thinking");
        };
        assert_eq!(thinking.signature, "");
        assert_eq!(thinking.thinking, "Condensed reasoning.");
    }

    #[test]
    fn redacted_thinking_block_serialization() {
        let redacted_block = RedactedThinkingBlock::new("encoded-thinking-data-123");
        let content_block = ContentBlock::from(redacted_block);

        let json = serde_json::to_string(&content_block).unwrap();
        let expected = r#"{"type":"redacted_thinking","data":"encoded-thinking-data-123"}"#;

        assert_eq!(json, expected);
    }

    #[test]
    fn deserialization() {
        let json = r#"{"text":"This is some text content.","type":"text"}"#;
        let content_block: ContentBlock = serde_json::from_str(json).unwrap();

        assert!(matches!(content_block, ContentBlock::Text(_)));
        assert!(!matches!(content_block, ContentBlock::ToolUse(_)));

        if let ContentBlock::Text(block) = content_block {
            assert_eq!(block.text, "This is some text content.");
        } else {
            panic!("Expected TextBlock");
        }

        let json = r#"{"id":"tool_123","input":{"url":"https://example.com"},"name":"web_fetch","type":"server_tool_use"}"#;
        let content_block: ContentBlock = serde_json::from_str(json).unwrap();

        assert!(matches!(content_block, ContentBlock::ServerToolUse(_)));
        assert!(!matches!(content_block, ContentBlock::Text(_)));

        if let ContentBlock::ServerToolUse(block) = content_block {
            assert_eq!(block.id, "tool_123");
            assert_eq!(block.name, "web_fetch");
        } else {
            panic!("Expected ServerToolUseBlock");
        }
    }

    #[test]
    fn as_methods() {
        let text_block = TextBlock::new("This is some text content.");
        let content_block = ContentBlock::from(text_block);

        assert!(content_block.as_text().is_some());
        assert!(content_block.as_tool_use().is_none());

        let text_ref = content_block.as_text().unwrap();
        assert_eq!(text_ref.text, "This is some text content.");
    }

    #[test]
    fn image_block_serialization() {
        let image_source = UrlImageSource::new("https://example.com/image.jpg");
        let image_block = ImageBlock::new(ImageSource::Url(image_source));
        let content_block = ContentBlock::from(image_block);

        let json = serde_json::to_string(&content_block).unwrap();
        let expected =
            r#"{"type":"image","source":{"type":"url","url":"https://example.com/image.jpg"}}"#;

        assert_eq!(json, expected);
    }

    fn round_trip(value: serde_json::Value) -> ContentBlock {
        let block: ContentBlock = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&block).unwrap(), value);
        block
    }

    #[test]
    fn web_fetch_tool_result_round_trip() {
        let block = round_trip(serde_json::json!({
            "type": "web_fetch_tool_result",
            "tool_use_id": "srvtoolu_01",
            "content": {
                "type": "web_fetch_result",
                "url": "https://example.com",
                "retrieved_at": "2026-10-19T00:00:00Z",
                "content": {
                    "type": "document",
                    "source": {"type": "text", "media_type": "text/plain", "data": "hello"},
                    "title": "Example"
                }
            }
        }));
        assert!(matches!(block, ContentBlock::WebFetchToolResult(_)));
        assert_eq!(block.label(), "[web_fetch_tool_result]");

        round_trip(serde_json::json!({
            "type": "web_fetch_tool_result",
            "tool_use_id": "srvtoolu_01",
            "content": {"type": "web_fetch_tool_result_error", "error_code": "url_not_accessible"}
        }));
    }

    #[test]
    fn code_execution_tool_result_round_trip() {
        let block = round_trip(serde_json::json!({
            "type": "code_execution_tool_result",
            "tool_use_id": "srvtoolu_02",
            "content": {
                "type": "code_execution_result",
                "stdout": "4\n",
                "stderr": "",
                "return_code": 0,
                "content": [{"type": "code_execution_output", "file_id": "file_01"}]
            }
        }));
        assert!(matches!(block, ContentBlock::CodeExecutionToolResult(_)));

        round_trip(serde_json::json!({
            "type": "code_execution_tool_result",
            "tool_use_id": "srvtoolu_02",
            "content": {"type": "code_execution_tool_result_error", "error_code": "unavailable"}
        }));
        round_trip(serde_json::json!({
            "type": "code_execution_tool_result",
            "tool_use_id": "srvtoolu_02",
            "content": {
                "type": "encrypted_code_execution_result",
                "encrypted_stdout": "abc",
                "stderr": "",
                "return_code": 0,
                "content": []
            }
        }));
    }

    #[test]
    fn opaque_code_execution_results_round_trip() {
        let bash = round_trip(serde_json::json!({
            "type": "bash_code_execution_tool_result",
            "tool_use_id": "srvtoolu_03",
            "content": {"type": "bash_code_execution_result", "stdout": "x", "return_code": 0}
        }));
        assert!(matches!(
            bash,
            ContentBlock::BashCodeExecutionToolResult { .. }
        ));
        let editor = round_trip(serde_json::json!({
            "type": "text_editor_code_execution_tool_result",
            "tool_use_id": "srvtoolu_04",
            "content": {"type": "text_editor_code_execution_view_result", "content": "x"}
        }));
        assert!(matches!(
            editor,
            ContentBlock::TextEditorCodeExecutionToolResult { .. }
        ));
    }

    #[test]
    fn tool_search_tool_result_round_trip() {
        let block = round_trip(serde_json::json!({
            "type": "tool_search_tool_result",
            "tool_use_id": "srvtoolu_05",
            "content": {
                "type": "tool_search_tool_search_result",
                "tool_references": [{"type": "tool_reference", "tool_name": "get_weather"}]
            }
        }));
        assert!(matches!(block, ContentBlock::ToolSearchToolResult(_)));
        round_trip(serde_json::json!({
            "type": "tool_search_tool_result",
            "tool_use_id": "srvtoolu_05",
            "content": {"type": "tool_search_tool_result_error", "error_code": "unavailable"}
        }));
        assert!(matches!(
            round_trip(serde_json::json!({"type": "tool_reference", "tool_name": "x"})),
            ContentBlock::ToolReference(_)
        ));
    }

    #[test]
    fn container_upload_round_trip() {
        let block =
            round_trip(serde_json::json!({"type": "container_upload", "file_id": "file_01"}));
        assert!(matches!(block, ContentBlock::ContainerUpload { .. }));
    }

    #[test]
    fn compaction_serializes_without_nulls() {
        let raw = r#"{"type":"compaction","content":"summary","signature":"sig"}"#;
        let block: ContentBlock = serde_json::from_str(raw).unwrap();
        assert!(matches!(block, ContentBlock::Compaction(_)));
        assert_eq!(serde_json::to_string(&block).unwrap(), raw);

        let with_nulls: ContentBlock = serde_json::from_str(
            r#"{"type":"compaction","content":"s","encrypted_content":null,"tool_changes":null}"#,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_string(&with_nulls).unwrap(),
            r#"{"type":"compaction","content":"s"}"#
        );
    }

    #[test]
    fn mcp_blocks_round_trip() {
        let call = round_trip(serde_json::json!({
            "type": "mcp_tool_use",
            "id": "mcptoolu_01",
            "name": "search",
            "server_name": "docs",
            "input": {"q": "x"}
        }));
        assert!(matches!(call, ContentBlock::McpToolUse(_)));
        let result = round_trip(serde_json::json!({
            "type": "mcp_tool_result",
            "tool_use_id": "mcptoolu_01",
            "content": [{"type": "text", "text": "found"}],
            "is_error": false
        }));
        assert!(matches!(result, ContentBlock::McpToolResult(_)));
        round_trip(serde_json::json!({
            "type": "mcp_tool_result",
            "tool_use_id": "mcptoolu_01",
            "content": "plain"
        }));
    }

    #[test]
    fn search_result_block_round_trip() {
        let block = round_trip(serde_json::json!({
            "type": "search_result",
            "source": "https://example.com/doc",
            "title": "Doc",
            "content": [{"type": "text", "text": "body"}],
            "citations": {"enabled": true}
        }));
        assert!(matches!(block, ContentBlock::SearchResult(_)));
    }

    #[test]
    fn unknown_block_round_trips_byte_for_byte() {
        let raw = r#"{"type":"some_future_block","foo":{"bar":[1,2]}}"#;
        let block: ContentBlock = serde_json::from_str(raw).unwrap();
        assert!(matches!(block, ContentBlock::Unknown(_)));
        assert_eq!(block.as_unknown().unwrap()["foo"]["bar"][1], 2);
        assert_eq!(block.label(), "[unknown block: some_future_block]");
        assert_eq!(serde_json::to_string(&block).unwrap(), raw);
    }

    #[test]
    fn unknown_block_round_trips_inside_message_and_param() {
        let raw_block = r#"{"type":"some_future_block","foo":{"bar":[1,2]}}"#;
        let message_json = format!(
            r#"{{"id":"msg_01","type":"message","role":"assistant","content":[{raw_block}],"model":"claude-sonnet-5-5","stop_reason":"end_turn","stop_sequence":null,"usage":{{"input_tokens":1,"output_tokens":1}}}}"#
        );
        let message: crate::Message = serde_json::from_str(&message_json).unwrap();
        let echoed = serde_json::to_value(&message).unwrap();
        assert_eq!(
            serde_json::to_string(&echoed["content"][0]).unwrap(),
            raw_block
        );

        let param_json = format!(r#"{{"role":"assistant","content":[{raw_block}]}}"#);
        let param: crate::MessageParam = serde_json::from_str(&param_json).unwrap();
        let echoed = serde_json::to_value(&param).unwrap();
        assert_eq!(
            serde_json::to_string(&echoed["content"][0]).unwrap(),
            raw_block
        );
    }

    #[test]
    fn malformed_known_block_still_errors() {
        let json = serde_json::json!({"type": "web_fetch_tool_result"});
        assert!(serde_json::from_value::<ContentBlock>(json).is_err());
    }

    #[test]
    fn search_result_location_citation_in_text_block() {
        let block = round_trip(serde_json::json!({
            "type": "text",
            "text": "cited",
            "citations": [{
                "type": "search_result_location",
                "cited_text": "c",
                "search_result_index": 0,
                "source": "s",
                "title": "t",
                "start_block_index": 0,
                "end_block_index": 1
            }]
        }));
        assert!(matches!(block, ContentBlock::Text(_)));
    }
}
