use serde::{Deserialize, Serialize};

use crate::types::ToolResultBlockContent;

/// A tool call the API made against a remote MCP server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct McpToolUseBlock {
    /// Unique identifier of this tool call.
    pub id: String,

    /// Name of the MCP tool.
    pub name: String,

    /// Name of the MCP server, as declared in `mcp_servers`.
    pub server_name: String,

    /// Input passed to the tool.
    pub input: serde_json::Value,
}

/// The result of an MCP tool call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct McpToolResultBlock {
    /// The ID of the `mcp_tool_use` block this result answers.
    pub tool_use_id: String,

    /// The tool output.
    pub content: ToolResultBlockContent,

    /// Whether the call failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}
