use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::types::CacheControlEphemeral;

/// Per-tool (or default) configuration inside an [`McpToolset`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct McpToolConfig {
    /// Whether the tool is enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,

    /// Whether the tool definition is loaded lazily via tool search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defer_loading: Option<bool>,
}

impl McpToolConfig {
    /// Enable or disable the tool.
    #[must_use]
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = Some(enabled);
        self
    }

    /// Load the tool definition lazily through tool search when `true`.
    #[must_use]
    pub fn with_defer_loading(mut self, defer_loading: bool) -> Self {
        self.defer_loading = Some(defer_loading);
        self
    }
}

/// Exposes the tools of an MCP server (declared in `mcp_servers`) to the model.
///
/// Requires the `mcp-client-2025-11-20` beta, which is added automatically.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct McpToolset {
    /// Name of the MCP server this toolset refers to.
    pub mcp_server_name: String,

    /// Configuration applied to every tool of the server unless overridden.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_config: Option<McpToolConfig>,

    /// Per-tool configuration keyed by tool name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configs: Option<BTreeMap<String, McpToolConfig>>,

    /// Create a cache control breakpoint at this content block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControlEphemeral>,
}

impl McpToolset {
    /// Creates a toolset for the named MCP server.
    #[must_use]
    pub fn new(mcp_server_name: impl Into<String>) -> Self {
        Self {
            mcp_server_name: mcp_server_name.into(),
            default_config: None,
            configs: None,
            cache_control: None,
        }
    }

    /// Sets the default configuration.
    #[must_use]
    pub fn with_default_config(mut self, config: McpToolConfig) -> Self {
        self.default_config = Some(config);
        self
    }

    /// Sets the per-tool configurations.
    #[must_use]
    pub fn with_configs(mut self, configs: BTreeMap<String, McpToolConfig>) -> Self {
        self.configs = Some(configs);
        self
    }

    /// Sets the cache control.
    #[must_use]
    pub fn with_cache_control(mut self, cache_control: CacheControlEphemeral) -> Self {
        self.cache_control = Some(cache_control);
        self
    }
}
