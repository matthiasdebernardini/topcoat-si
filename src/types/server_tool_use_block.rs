use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::CacheControlEphemeral;

/// A block representing a server-side tool use request from the model.
///
/// ServerToolUseBlocks indicate the model wants to use a server-side tool (like web fetch).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct ServerToolUseBlock {
    /// A unique identifier for this tool use request.
    pub id: String,

    /// The input data for the tool, can be any valid JSON.
    pub input: Value,

    /// The name of the server tool being invoked, such as "web_fetch" or "code_execution".
    pub name: String,

    /// Create a cache control breakpoint at this content block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControlEphemeral>,
}

impl ServerToolUseBlock {
    /// Creates a new ServerToolUseBlock with the specified id, tool name and input.
    #[must_use]
    pub fn new(id: impl Into<String>, name: impl Into<String>, input: Value) -> Self {
        Self {
            id: id.into(),
            input,
            name: name.into(),
            cache_control: None,
        }
    }

    /// Add a cache control to this server tool use block.
    #[must_use]
    pub fn with_cache_control(mut self, cache_control: CacheControlEphemeral) -> Self {
        self.cache_control = Some(cache_control);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

    #[test]
    fn server_tool_use_block_serialization() {
        let input_json = serde_json::json!({
            "url": "https://example.com"
        });

        let block = ServerToolUseBlock::new("tool_123", "web_fetch", input_json);

        let json = serde_json::to_string(&block).unwrap();
        let expected =
            r#"{"id":"tool_123","input":{"url":"https://example.com"},"name":"web_fetch"}"#;

        assert_eq!(json, expected);
    }

    #[test]
    fn deserialization() {
        let json = r#"{"id":"tool_123","input":{"url":"https://example.com"},"name":"web_fetch"}"#;
        let block: ServerToolUseBlock = serde_json::from_str(json).unwrap();

        assert_eq!(block.id, "tool_123");
        assert_eq!(block.name, "web_fetch");

        let expected_input = serde_json::json!({
            "url": "https://example.com"
        });
        assert_eq!(block.input, expected_input);
    }

    #[test]
    fn server_tool_use_block_with_cache_control() {
        let input = json!({
            "url": "https://example.com"
        });

        let cache_control = CacheControlEphemeral::new();
        let block = ServerToolUseBlock::new("server_tool_1", "web_fetch", input)
            .with_cache_control(cache_control);

        let json = to_value(&block).unwrap();
        assert_eq!(
            json,
            json!({
                "id": "server_tool_1",
                "input": {
                    "url": "https://example.com"
                },
                "name": "web_fetch",
                "cache_control": {
                    "type": "ephemeral"
                }
            })
        );
    }
}
