use serde::{Deserialize, Serialize};

use crate::types::CacheControlEphemeral;

/// Parameters for a tool search server tool (regex or BM25 variant).
///
/// The variant is chosen by the [`ToolUnionParam`](crate::ToolUnionParam) variant;
/// the matching constructor sets the expected name.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolSearchTool {
    /// Name of the tool: `tool_search_tool_regex` or `tool_search_tool_bm25`.
    pub name: String,

    /// Create a cache control breakpoint at this content block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControlEphemeral>,
}

impl ToolSearchTool {
    /// Creates a tool search tool with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            cache_control: None,
        }
    }

    /// Creates the regex variant (`tool_search_tool_regex`).
    pub fn regex() -> Self {
        Self::new("tool_search_tool_regex")
    }

    /// Creates the BM25 variant (`tool_search_tool_bm25`).
    pub fn bm25() -> Self {
        Self::new("tool_search_tool_bm25")
    }

    /// Sets the cache control.
    pub fn with_cache_control(mut self, cache_control: CacheControlEphemeral) -> Self {
        self.cache_control = Some(cache_control);
        self
    }
}
