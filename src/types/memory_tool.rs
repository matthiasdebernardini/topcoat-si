use serde::{Deserialize, Serialize};

use crate::types::CacheControlEphemeral;

fn default_name() -> String {
    "memory".to_string()
}

/// Parameters for the memory tool (`memory_20250818`).
///
/// This is a client-side tool: the model emits `tool_use` blocks carrying memory
/// commands (`view`, `create`, `str_replace`, `insert`, `delete`, `rename`) and the
/// application implements them and returns the results.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryTool {
    /// Name of the tool.
    #[serde(default = "default_name")]
    pub name: String,

    /// Create a cache control breakpoint at this content block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControlEphemeral>,
}

impl MemoryTool {
    /// Creates a memory tool with default values.
    pub fn new() -> Self {
        Self {
            name: default_name(),
            cache_control: None,
        }
    }

    /// Sets the cache control.
    pub fn with_cache_control(mut self, cache_control: CacheControlEphemeral) -> Self {
        self.cache_control = Some(cache_control);
        self
    }
}

impl Default for MemoryTool {
    fn default() -> Self {
        Self::new()
    }
}
