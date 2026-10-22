use serde::{Deserialize, Serialize};

use crate::types::{
    BashTool, CodeExecutionTool, McpToolset, MemoryTool, TextEditorTool, ToolParam, ToolSearchTool,
    WebFetchTool,
};

/// Union type for different tool parameter types.
///
/// This type represents a union of different tool types that can be used with Claude, including:
/// - Custom tools
/// - Bash tools
/// - Text editor tools
/// - Server tools (web fetch, code execution, tool search)
/// - The client-side memory tool
/// - MCP toolsets
///
/// The API accepts any of these tool types when tools are provided to Claude. Build one with its
/// variant and the tool type's constructor, for example
/// `ToolUnionParam::WebFetch20260318(WebFetchTool::new())`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum ToolUnionParam {
    /// A custom tool with a defined schema
    #[serde(rename = "custom")]
    CustomTool(ToolParam),

    /// A bash tool for executing shell commands (version 20241022)
    #[serde(rename = "bash_20241022")]
    Bash20241022(BashTool),

    /// A bash tool for executing shell commands (version 20250124)
    #[serde(rename = "bash_20250124")]
    Bash20250124(BashTool),

    /// A text editor tool for making changes to text
    #[serde(rename = "text_editor_20250124")]
    TextEditor20250124(TextEditorTool),

    /// A text editor tool for making changes to text (version 20250429)
    #[serde(rename = "text_editor_20250429")]
    TextEditor20250429(TextEditorTool),

    /// A text editor tool for making changes to text (version 20250728)
    #[serde(rename = "text_editor_20250728")]
    TextEditor20250728(TextEditorTool),

    /// A web fetch tool (version 20250910)
    #[serde(rename = "web_fetch_20250910")]
    WebFetch20250910(WebFetchTool),

    /// A web fetch tool (version 20260209)
    #[serde(rename = "web_fetch_20260209")]
    WebFetch20260209(WebFetchTool),

    /// A web fetch tool (version 20260309)
    #[serde(rename = "web_fetch_20260309")]
    WebFetch20260309(WebFetchTool),

    /// A web fetch tool (version 20260318)
    #[serde(rename = "web_fetch_20260318")]
    WebFetch20260318(WebFetchTool),

    /// A code execution tool (version 20250825)
    #[serde(rename = "code_execution_20250825")]
    CodeExecution20250825(CodeExecutionTool),

    /// A code execution tool (version 20260120)
    #[serde(rename = "code_execution_20260120")]
    CodeExecution20260120(CodeExecutionTool),

    /// A code execution tool (version 20260521)
    #[serde(rename = "code_execution_20260521")]
    CodeExecution20260521(CodeExecutionTool),

    /// A regex-based tool search tool
    #[serde(rename = "tool_search_tool_regex_20251119")]
    ToolSearchToolRegex20251119(ToolSearchTool),

    /// A BM25-based tool search tool
    #[serde(rename = "tool_search_tool_bm25_20251119")]
    ToolSearchToolBm2520251119(ToolSearchTool),

    /// The client-side memory tool
    #[serde(rename = "memory_20250818")]
    Memory20250818(MemoryTool),

    /// A toolset exposing the tools of an MCP server
    #[serde(rename = "mcp_toolset")]
    McpToolset(McpToolset),
}

impl ToolUnionParam {
    /// Creates a new custom tool
    #[must_use]
    pub fn new_custom_tool(name: impl Into<String>, input_schema: serde_json::Value) -> Self {
        Self::CustomTool(ToolParam::new(name, input_schema))
    }

    /// Validate tool-specific constraints.
    ///
    /// `allowed_domains` and `blocked_domains` are mutually exclusive on web fetch tools.
    pub fn validate(&self) -> Result<(), crate::Error> {
        let (name, allowed, blocked) = match self {
            Self::WebFetch20250910(tool)
            | Self::WebFetch20260209(tool)
            | Self::WebFetch20260309(tool)
            | Self::WebFetch20260318(tool) => (
                &tool.name,
                tool.allowed_domains.is_some(),
                tool.blocked_domains.is_some(),
            ),
            _ => return Ok(()),
        };
        if allowed && blocked {
            return Err(crate::Error::validation(
                format!("tool {name}: allowed_domains and blocked_domains are mutually exclusive"),
                Some("tools".to_string()),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CacheControlEphemeral;
    use serde_json::{json, to_value};

    #[test]
    fn custom_tool() {
        let input_schema = json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                }
            }
        });

        let custom_tool = ToolParam::new("search", input_schema)
            .with_description("Search for information")
            .with_cache_control(CacheControlEphemeral::new());

        let tool = ToolUnionParam::CustomTool(custom_tool);

        let json = to_value(&tool).unwrap();
        assert_eq!(
            json,
            json!({
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "The search query"
                        }
                    }
                },
                "name": "search",
                "cache_control": {
                    "type": "ephemeral"
                },
                "description": "Search for information",
                "type": "custom"
            })
        );
    }

    #[test]
    fn bash_20241022_tool() {
        let bash_tool = BashTool::new().with_ephemeral_cache_control();
        let tool = ToolUnionParam::Bash20241022(bash_tool);

        let json = to_value(&tool).unwrap();
        assert_eq!(
            json,
            json!({
                "name": "bash",
                "type": "bash_20241022",
                "cache_control": {
                    "type": "ephemeral"
                }
            })
        );
    }

    #[test]
    fn bash_20250124_tool() {
        let bash_tool = BashTool::new().with_ephemeral_cache_control();
        let tool = ToolUnionParam::Bash20250124(bash_tool);

        let json = to_value(&tool).unwrap();
        assert_eq!(
            json,
            json!({
                "name": "bash",
                "type": "bash_20250124",
                "cache_control": {
                    "type": "ephemeral"
                }
            })
        );
    }

    #[test]
    fn text_editor_tool() {
        let text_editor_tool = TextEditorTool::new_20250124().with_ephemeral_cache_control();
        let tool = ToolUnionParam::TextEditor20250124(text_editor_tool);

        let json = to_value(&tool).unwrap();
        assert_eq!(
            json,
            json!({
                "name": "str_replace_editor",
                "type": "text_editor_20250124",
                "cache_control": {
                    "type": "ephemeral"
                }
            })
        );
    }

    #[test]
    fn text_editor_20250429_tool() {
        let text_editor_tool = TextEditorTool::new_20250429().with_ephemeral_cache_control();
        let tool = ToolUnionParam::TextEditor20250429(text_editor_tool);

        let json = to_value(&tool).unwrap();
        assert_eq!(
            json,
            json!({
                "name": "str_replace_based_edit_tool",
                "type": "text_editor_20250429",
                "cache_control": {
                    "type": "ephemeral"
                }
            })
        );
    }

    #[test]
    fn text_editor_20250728_tool() {
        let text_editor_tool = TextEditorTool::new_20250728()
            .with_max_characters(5000)
            .with_ephemeral_cache_control();
        let tool = ToolUnionParam::TextEditor20250728(text_editor_tool);

        let json = to_value(&tool).unwrap();
        assert_eq!(
            json,
            json!({
                "name": "str_replace_based_edit_tool",
                "type": "text_editor_20250728",
                "cache_control": {
                    "type": "ephemeral"
                },
                "max_characters": 5000
            })
        );
    }

    #[test]
    fn deserialization() {
        // Test custom tool deserialization
        let json = json!({
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query"
                    }
                }
            },
            "name": "search",
            "description": "Search for information",
            "type": "custom"
        });

        let tool: ToolUnionParam = serde_json::from_value(json).unwrap();
        match tool {
            ToolUnionParam::CustomTool(t) => {
                assert_eq!(t.name, "search");
                assert_eq!(t.description, Some("Search for information".to_string()));
            }
            _ => panic!("Expected CustomTool variant"),
        }

        // Test bash 20241022 tool deserialization
        let json = json!({
            "name": "bash",
            "type": "bash_20241022"
        });

        let tool: ToolUnionParam = serde_json::from_value(json).unwrap();
        match tool {
            ToolUnionParam::Bash20241022(t) => {
                assert_eq!(t.name, "bash");
            }
            _ => panic!("Expected Bash20241022 variant"),
        }

        // Test bash 20250124 tool deserialization
        let json = json!({
            "name": "bash",
            "type": "bash_20250124"
        });

        let tool: ToolUnionParam = serde_json::from_value(json).unwrap();
        match tool {
            ToolUnionParam::Bash20250124(t) => {
                assert_eq!(t.name, "bash");
            }
            _ => panic!("Expected Bash20250124 variant"),
        }
    }

    #[test]
    fn server_and_client_tool_variants() {
        let cases = [
            (
                ToolUnionParam::WebFetch20250910(WebFetchTool::new()),
                json!({"type": "web_fetch_20250910", "name": "web_fetch"}),
            ),
            (
                ToolUnionParam::WebFetch20260209(WebFetchTool::new()),
                json!({"type": "web_fetch_20260209", "name": "web_fetch"}),
            ),
            (
                ToolUnionParam::WebFetch20260309(WebFetchTool::new().with_use_cache(true)),
                json!({"type": "web_fetch_20260309", "name": "web_fetch", "use_cache": true}),
            ),
            (
                ToolUnionParam::WebFetch20260318(WebFetchTool::new()),
                json!({"type": "web_fetch_20260318", "name": "web_fetch"}),
            ),
            (
                ToolUnionParam::CodeExecution20250825(CodeExecutionTool::new()),
                json!({"type": "code_execution_20250825", "name": "code_execution"}),
            ),
            (
                ToolUnionParam::CodeExecution20260120(CodeExecutionTool::new()),
                json!({"type": "code_execution_20260120", "name": "code_execution"}),
            ),
            (
                ToolUnionParam::CodeExecution20260521(CodeExecutionTool::new()),
                json!({"type": "code_execution_20260521", "name": "code_execution"}),
            ),
            (
                ToolUnionParam::ToolSearchToolRegex20251119(ToolSearchTool::regex()),
                json!({"type": "tool_search_tool_regex_20251119", "name": "tool_search_tool_regex"}),
            ),
            (
                ToolUnionParam::ToolSearchToolBm2520251119(ToolSearchTool::bm25()),
                json!({"type": "tool_search_tool_bm25_20251119", "name": "tool_search_tool_bm25"}),
            ),
            (
                ToolUnionParam::Memory20250818(MemoryTool::new()),
                json!({"type": "memory_20250818", "name": "memory"}),
            ),
            (
                ToolUnionParam::McpToolset(McpToolset::new("docs")),
                json!({"type": "mcp_toolset", "mcp_server_name": "docs"}),
            ),
        ];
        for (tool, expected) in cases {
            assert_eq!(to_value(&tool).unwrap(), expected);
            let parsed: ToolUnionParam = serde_json::from_value(expected).unwrap();
            assert_eq!(parsed, tool);
        }
    }

    #[test]
    fn validate_rejects_both_domain_lists() {
        let mut tool = WebFetchTool::new().with_allowed_domains(vec!["a.com".to_string()]);
        tool.blocked_domains = Some(vec!["b.com".to_string()]);
        let err = ToolUnionParam::WebFetch20260318(tool)
            .validate()
            .unwrap_err();
        assert!(matches!(err.kind(), crate::ErrorKind::Validation { .. }));
        assert!(
            ToolUnionParam::WebFetch20260318(WebFetchTool::new())
                .validate()
                .is_ok()
        );
    }
}
