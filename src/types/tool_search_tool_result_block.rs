use serde::{Deserialize, Serialize};

use crate::types::open_union::with_type_seq;

/// A reference to a tool discovered by tool search.
///
/// The `type` tag (`tool_reference`) is supplied by the enclosing union.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolReference {
    /// Name of the referenced tool.
    pub tool_name: String,
}

impl ToolReference {
    /// Creates a reference to the named tool.
    pub fn new(tool_name: impl Into<String>) -> Self {
        Self {
            tool_name: tool_name.into(),
        }
    }
}

fn serialize_references<S>(references: &[ToolReference], serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    with_type_seq("tool_reference", references, serializer)
}

/// The outcome of a tool search.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ToolSearchToolResultContent {
    /// Tools matched the search.
    #[serde(rename = "tool_search_tool_search_result")]
    Result {
        /// The matching tools.
        #[serde(serialize_with = "serialize_references")]
        tool_references: Vec<ToolReference>,
    },
    /// The search failed.
    #[serde(rename = "tool_search_tool_result_error")]
    Error {
        /// The error code reported by the API.
        error_code: String,
    },
}

/// The result of a server-side tool search.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolSearchToolResultBlock {
    /// The ID of the `server_tool_use` block this result answers.
    pub tool_use_id: String,

    /// The search result or error.
    pub content: ToolSearchToolResultContent,
}
