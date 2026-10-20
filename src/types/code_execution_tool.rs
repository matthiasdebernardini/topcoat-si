use serde::{Deserialize, Serialize};

use crate::types::CacheControlEphemeral;

fn default_name() -> String {
    "code_execution".to_string()
}

/// Parameters for the code execution server tool.
///
/// The same parameters are shared by every `code_execution_*` version; the version is
/// chosen by the [`ToolUnionParam`](crate::ToolUnionParam) variant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodeExecutionTool {
    /// Name of the tool.
    #[serde(default = "default_name")]
    pub name: String,

    /// Create a cache control breakpoint at this content block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControlEphemeral>,

    /// Whether the tool definition is loaded lazily via tool search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defer_loading: Option<bool>,
}

impl CodeExecutionTool {
    /// Creates a code execution tool with default values.
    pub fn new() -> Self {
        Self {
            name: default_name(),
            cache_control: None,
            defer_loading: None,
        }
    }

    /// Sets the cache control.
    pub fn with_cache_control(mut self, cache_control: CacheControlEphemeral) -> Self {
        self.cache_control = Some(cache_control);
        self
    }

    /// Sets whether the tool definition is deferred until found by tool search.
    pub fn with_defer_loading(mut self, defer_loading: bool) -> Self {
        self.defer_loading = Some(defer_loading);
        self
    }
}

impl Default for CodeExecutionTool {
    fn default() -> Self {
        Self::new()
    }
}
