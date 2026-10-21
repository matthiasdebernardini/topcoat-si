use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::Value;

use crate::types::open_union;

/// A reference to a tool discovered by tool search.
///
/// The `type` tag (`tool_reference`) is supplied by the enclosing union.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolReference {
    /// Name of the referenced tool.
    pub tool_name: String,
}

impl ToolReference {
    /// Creates a reference to the named tool.
    #[must_use]
    pub fn new(tool_name: impl Into<String>) -> Self {
        Self {
            tool_name: tool_name.into(),
        }
    }
}

fn serialize_references<S>(references: &[ToolReference], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    open_union::with_type_seq("tool_reference", references, serializer)
}

/// The outcome of a tool search.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type")]
#[non_exhaustive]
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
    /// An outcome of a type this crate does not know, kept verbatim.
    #[serde(untagged)]
    Unknown(Value),
}

impl<'de> Deserialize<'de> for ToolSearchToolResultContent {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "type")]
        enum Known {
            #[serde(rename = "tool_search_tool_search_result")]
            Result { tool_references: Vec<ToolReference> },
            #[serde(rename = "tool_search_tool_result_error")]
            Error { error_code: String },
            #[serde(other)]
            Other,
        }

        let value = Value::deserialize(deserializer)?;
        Ok(
            match Known::deserialize(&value).map_err(de::Error::custom)? {
                Known::Result { tool_references } => Self::Result { tool_references },
                Known::Error { error_code } => Self::Error { error_code },
                Known::Other => {
                    open_union::warn_unknown_member("ToolSearchToolResultContent", &value);
                    Self::Unknown(value)
                }
            },
        )
    }
}

/// The result of a server-side tool search.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct ToolSearchToolResultBlock {
    /// The ID of the `server_tool_use` block this result answers.
    pub tool_use_id: String,

    /// The search result or error.
    pub content: ToolSearchToolResultContent,
}
