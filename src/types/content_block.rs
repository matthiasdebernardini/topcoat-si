use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

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
            .ok_or_else(|| serde::de::Error::missing_field("type"))?
            .to_string();

        match block_type.as_str() {
            "text" => from_value(value).map(ContentBlock::Text),
            "image" => from_value(value).map(ContentBlock::Image),
            "tool_use" => from_value(value).map(ContentBlock::ToolUse),
            "server_tool_use" => from_value(value).map(ContentBlock::ServerToolUse),
            "tool_result" => from_value(value).map(ContentBlock::ToolResult),
            "document" => from_value(value).map(ContentBlock::Document),
            "thinking" | "thinking_summary" | "summary" => {
                from_value(value).map(ContentBlock::Thinking)
            }
            "redacted_thinking" => from_value(value).map(ContentBlock::RedactedThinking),
            "fallback" => from_value(value).map(ContentBlock::Fallback),
            "search_result" => from_value(value).map(ContentBlock::SearchResult),
            "web_fetch_tool_result" => from_value(value).map(ContentBlock::WebFetchToolResult),
            "code_execution_tool_result" => {
                from_value(value).map(ContentBlock::CodeExecutionToolResult)
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
            "tool_search_tool_result" => from_value(value).map(ContentBlock::ToolSearchToolResult),
            "container_upload" => {
                #[derive(Deserialize)]
                struct Upload {
                    file_id: String,
                }
                from_value::<Upload, D::Error>(value).map(|upload| ContentBlock::ContainerUpload {
                    file_id: upload.file_id,
                })
            }
            "compaction" => from_value(value).map(ContentBlock::Compaction),
            "mcp_tool_use" => from_value(value).map(ContentBlock::McpToolUse),
            "mcp_tool_result" => from_value(value).map(ContentBlock::McpToolResult),
            "tool_reference" => from_value(value).map(ContentBlock::ToolReference),
            _ => Ok(ContentBlock::Unknown(value)),
        }
    }
}

/// Reads the fields of a block whose `content` is kept opaque.
fn opaque_result<E: serde::de::Error>(value: Value) -> Result<(String, Value), E> {
    #[derive(Deserialize)]
    struct Opaque {
        tool_use_id: String,
        content: Value,
    }
    from_value::<Opaque, E>(value).map(|opaque| (opaque.tool_use_id, opaque.content))
}

fn from_value<T, E>(value: Value) -> Result<T, E>
where
    T: serde::de::DeserializeOwned,
    E: serde::de::Error,
{
    serde_json::from_value(value).map_err(E::custom)
}

impl ContentBlock {
    /// Returns true if this block is a text block
    pub fn is_text(&self) -> bool {
        matches!(self, ContentBlock::Text(_))
    }

    /// Returns true if this block is an image block
    pub fn is_image(&self) -> bool {
        matches!(self, ContentBlock::Image(_))
    }

    /// Returns true if this block is a tool use block
    pub fn is_tool_use(&self) -> bool {
        matches!(self, ContentBlock::ToolUse(_))
    }

    /// Returns true if this block is a server tool use block
    pub fn is_server_tool_use(&self) -> bool {
        matches!(self, ContentBlock::ServerToolUse(_))
    }

    /// Returns true if this block is a tool result block
    pub fn is_tool_result(&self) -> bool {
        matches!(self, ContentBlock::ToolResult(_))
    }

    /// Returns true if this block is a document block
    pub fn is_document(&self) -> bool {
        matches!(self, ContentBlock::Document(_))
    }

    /// Returns true if this block is a thinking block
    pub fn is_thinking(&self) -> bool {
        matches!(self, ContentBlock::Thinking(_))
    }

    /// Returns true if this block is a redacted thinking block
    pub fn is_redacted_thinking(&self) -> bool {
        matches!(self, ContentBlock::RedactedThinking(_))
    }

    /// Returns true if this block marks a model fallback boundary.
    pub fn is_fallback(&self) -> bool {
        matches!(self, ContentBlock::Fallback(_))
    }

    /// Returns true if this block is a search result block
    pub fn is_search_result(&self) -> bool {
        matches!(self, ContentBlock::SearchResult(_))
    }

    /// Returns true if this block is a web fetch tool result block
    pub fn is_web_fetch_tool_result(&self) -> bool {
        matches!(self, ContentBlock::WebFetchToolResult(_))
    }

    /// Returns true if this block is a code execution tool result block
    pub fn is_code_execution_tool_result(&self) -> bool {
        matches!(self, ContentBlock::CodeExecutionToolResult(_))
    }

    /// Returns true if this block is a bash code execution tool result block
    pub fn is_bash_code_execution_tool_result(&self) -> bool {
        matches!(self, ContentBlock::BashCodeExecutionToolResult { .. })
    }

    /// Returns true if this block is a text editor code execution tool result block
    pub fn is_text_editor_code_execution_tool_result(&self) -> bool {
        matches!(self, ContentBlock::TextEditorCodeExecutionToolResult { .. })
    }

    /// Returns true if this block is a tool search tool result block
    pub fn is_tool_search_tool_result(&self) -> bool {
        matches!(self, ContentBlock::ToolSearchToolResult(_))
    }

    /// Returns true if this block is a container upload block
    pub fn is_container_upload(&self) -> bool {
        matches!(self, ContentBlock::ContainerUpload { .. })
    }

    /// Returns true if this block is a compaction block
    pub fn is_compaction(&self) -> bool {
        matches!(self, ContentBlock::Compaction(_))
    }

    /// Returns true if this block is an MCP tool use block
    pub fn is_mcp_tool_use(&self) -> bool {
        matches!(self, ContentBlock::McpToolUse(_))
    }

    /// Returns true if this block is an MCP tool result block
    pub fn is_mcp_tool_result(&self) -> bool {
        matches!(self, ContentBlock::McpToolResult(_))
    }

    /// Returns true if this block is a tool reference block
    pub fn is_tool_reference(&self) -> bool {
        matches!(self, ContentBlock::ToolReference(_))
    }

    /// Returns true if this block has a type this crate does not know
    pub fn is_unknown(&self) -> bool {
        matches!(self, ContentBlock::Unknown(_))
    }

    /// Returns the raw JSON if this is an unrecognized block, or None otherwise.
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
    pub fn label(&self) -> String {
        match self {
            ContentBlock::Unknown(_) => format!("[unknown block: {}]", self.type_name()),
            other => format!("[{}]", other.type_name()),
        }
    }

    /// Returns a reference to the inner TextBlock if this is a Text variant,
    /// or None otherwise.
    pub fn as_text(&self) -> Option<&TextBlock> {
        match self {
            ContentBlock::Text(block) => Some(block),
            _ => None,
        }
    }

    /// Returns a reference to the inner ImageBlock if this is an Image variant,
    /// or None otherwise.
    pub fn as_image(&self) -> Option<&ImageBlock> {
        match self {
            ContentBlock::Image(block) => Some(block),
            _ => None,
        }
    }

    /// Returns a reference to the inner ToolUseBlock if this is a ToolUse variant,
    /// or None otherwise.
    pub fn as_tool_use(&self) -> Option<&ToolUseBlock> {
        match self {
            ContentBlock::ToolUse(block) => Some(block),
            _ => None,
        }
    }

    /// Returns a reference to the inner ServerToolUseBlock if this is a ServerToolUse variant,
    /// or None otherwise.
    pub fn as_server_tool_use(&self) -> Option<&ServerToolUseBlock> {
        match self {
            ContentBlock::ServerToolUse(block) => Some(block),
            _ => None,
        }
    }

    /// Returns a reference to the inner ToolResultBlock if this is a ToolResult variant,
    /// or None otherwise.
    pub fn as_tool_result(&self) -> Option<&ToolResultBlock> {
        match self {
            ContentBlock::ToolResult(block) => Some(block),
            _ => None,
        }
    }

    /// Returns a reference to the inner DocumentBlock if this is a Document variant,
    /// or None otherwise.
    pub fn as_document(&self) -> Option<&DocumentBlock> {
        match self {
            ContentBlock::Document(block) => Some(block),
            _ => None,
        }
    }

    /// Returns a reference to the inner ThinkingBlock if this is a Thinking variant,
    /// or None otherwise.
    pub fn as_thinking(&self) -> Option<&ThinkingBlock> {
        match self {
            ContentBlock::Thinking(block) => Some(block),
            _ => None,
        }
    }

    /// Returns a reference to the inner RedactedThinkingBlock if this is a RedactedThinking variant,
    /// or None otherwise.
    pub fn as_redacted_thinking(&self) -> Option<&RedactedThinkingBlock> {
        match self {
            ContentBlock::RedactedThinking(block) => Some(block),
            _ => None,
        }
    }
}

/// Helper methods to create ContentBlock variants
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
        assert_eq!(
            block.from.model,
            crate::types::Model::Known(crate::types::KnownModel::ClaudeFable51)
        );
        assert_eq!(
            block.to.model,
            crate::types::Model::Known(crate::types::KnownModel::ClaudeOpus55)
        );
    }

    #[test]
    fn thinking_summary_block_deserializes_as_thinking() {
        let json = r#"{"type":"thinking_summary","summary":"Condensed reasoning."}"#;
        let content_block: ContentBlock = serde_json::from_str(json).unwrap();

        let thinking = content_block
            .as_thinking()
            .expect("summary block should deserialize as thinking");
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

        assert!(content_block.is_text());
        assert!(!content_block.is_tool_use());

        if let ContentBlock::Text(block) = content_block {
            assert_eq!(block.text, "This is some text content.");
        } else {
            panic!("Expected TextBlock");
        }

        let json = r#"{"id":"tool_123","input":{"url":"https://example.com"},"name":"web_fetch","type":"server_tool_use"}"#;
        let content_block: ContentBlock = serde_json::from_str(json).unwrap();

        assert!(content_block.is_server_tool_use());
        assert!(!content_block.is_text());

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
        assert!(content_block.as_image().is_none());
        assert!(content_block.as_tool_use().is_none());
        assert!(content_block.as_server_tool_use().is_none());
        assert!(content_block.as_tool_result().is_none());
        assert!(content_block.as_document().is_none());
        assert!(content_block.as_thinking().is_none());
        assert!(content_block.as_redacted_thinking().is_none());

        let text_ref = content_block.as_text().unwrap();
        assert_eq!(text_ref.text, "This is some text content.");
    }

    #[test]
    fn image_block_serialization() {
        let image_source =
            crate::types::UrlImageSource::new("https://example.com/image.jpg".to_string());
        let image_block = ImageBlock::new_with_url(image_source);
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
        assert!(block.is_web_fetch_tool_result());
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
        assert!(block.is_code_execution_tool_result());

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
        assert!(bash.is_bash_code_execution_tool_result());
        let editor = round_trip(serde_json::json!({
            "type": "text_editor_code_execution_tool_result",
            "tool_use_id": "srvtoolu_04",
            "content": {"type": "text_editor_code_execution_view_result", "content": "x"}
        }));
        assert!(editor.is_text_editor_code_execution_tool_result());
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
        assert!(block.is_tool_search_tool_result());
        round_trip(serde_json::json!({
            "type": "tool_search_tool_result",
            "tool_use_id": "srvtoolu_05",
            "content": {"type": "tool_search_tool_result_error", "error_code": "unavailable"}
        }));
        assert!(
            round_trip(serde_json::json!({"type": "tool_reference", "tool_name": "x"}))
                .is_tool_reference()
        );
    }

    #[test]
    fn container_upload_round_trip() {
        let block =
            round_trip(serde_json::json!({"type": "container_upload", "file_id": "file_01"}));
        assert!(block.is_container_upload());
    }

    #[test]
    fn compaction_serializes_without_nulls() {
        let raw = r#"{"type":"compaction","content":"summary","signature":"sig"}"#;
        let block: ContentBlock = serde_json::from_str(raw).unwrap();
        assert!(block.is_compaction());
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
        assert!(call.is_mcp_tool_use());
        let result = round_trip(serde_json::json!({
            "type": "mcp_tool_result",
            "tool_use_id": "mcptoolu_01",
            "content": [{"type": "text", "text": "found"}],
            "is_error": false
        }));
        assert!(result.is_mcp_tool_result());
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
        assert!(block.is_search_result());
    }

    #[test]
    fn unknown_block_round_trips_byte_for_byte() {
        let raw = r#"{"type":"some_future_block","foo":{"bar":[1,2]}}"#;
        let block: ContentBlock = serde_json::from_str(raw).unwrap();
        assert!(block.is_unknown());
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
        assert!(block.is_text());
    }
}
