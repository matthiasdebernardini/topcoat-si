use serde::{Deserialize, Serialize};

use crate::types::{CacheControlEphemeral, CitationsConfig, ResponseInclusion};

fn default_name() -> String {
    "web_fetch".to_string()
}

/// Parameters for the web fetch server tool.
///
/// The same parameters are shared by every `web_fetch_*` version; the version is chosen
/// by the [`ToolUnionParam`](crate::ToolUnionParam) variant. `allowed_domains` and
/// `blocked_domains` are mutually exclusive; the builders enforce this.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WebFetchTool {
    /// Name of the tool.
    #[serde(default = "default_name")]
    pub name: String,

    /// If provided, only these domains may be fetched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_domains: Option<Vec<String>>,

    /// If provided, these domains are never fetched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_domains: Option<Vec<String>>,

    /// Citation configuration for fetched documents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citations: Option<CitationsConfig>,

    /// Maximum number of content tokens included from a fetched page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_content_tokens: Option<u64>,

    /// Maximum number of times the tool can be used in the API request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_uses: Option<u32>,

    /// Whether cached page content may be used (`web_fetch_20260309` and later).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_cache: Option<bool>,

    /// Whether fetched content is included in full or excluded (`web_fetch_20260318`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_inclusion: Option<ResponseInclusion>,

    /// Create a cache control breakpoint at this content block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControlEphemeral>,

    /// Whether the tool definition is loaded lazily via tool search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defer_loading: Option<bool>,

    /// Callers allowed to invoke this tool, such as `"direct"` or a code execution version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_callers: Option<Vec<String>>,
}

impl WebFetchTool {
    /// Creates a web fetch tool with default values.
    pub fn new() -> Self {
        Self {
            name: default_name(),
            allowed_domains: None,
            blocked_domains: None,
            citations: None,
            max_content_tokens: None,
            max_uses: None,
            use_cache: None,
            response_inclusion: None,
            cache_control: None,
            defer_loading: None,
            allowed_callers: None,
        }
    }

    /// Only allow these domains; clears `blocked_domains`.
    pub fn with_allowed_domains(mut self, domains: Vec<String>) -> Self {
        self.allowed_domains = Some(domains);
        self.blocked_domains = None;
        self
    }

    /// Block these domains; clears `allowed_domains`.
    pub fn with_blocked_domains(mut self, domains: Vec<String>) -> Self {
        self.blocked_domains = Some(domains);
        self.allowed_domains = None;
        self
    }

    /// Sets the citation configuration.
    pub fn with_citations(mut self, citations: CitationsConfig) -> Self {
        self.citations = Some(citations);
        self
    }

    /// Sets the maximum content tokens per fetched page.
    pub fn with_max_content_tokens(mut self, max_content_tokens: u64) -> Self {
        self.max_content_tokens = Some(max_content_tokens);
        self
    }

    /// Sets the maximum number of uses per request.
    pub fn with_max_uses(mut self, max_uses: u32) -> Self {
        self.max_uses = Some(max_uses);
        self
    }

    /// Sets whether cached content may be used.
    pub fn with_use_cache(mut self, use_cache: bool) -> Self {
        self.use_cache = Some(use_cache);
        self
    }

    /// Sets whether fetched content is included in full or excluded.
    pub fn with_response_inclusion(mut self, response_inclusion: ResponseInclusion) -> Self {
        self.response_inclusion = Some(response_inclusion);
        self
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

    /// Sets the callers allowed to invoke this tool.
    pub fn with_allowed_callers(mut self, allowed_callers: Vec<String>) -> Self {
        self.allowed_callers = Some(allowed_callers);
        self
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serializes_all_fields() {
        let tool = WebFetchTool::new()
            .with_blocked_domains(vec!["bad.example".to_string()])
            .with_citations(CitationsConfig::enabled())
            .with_max_content_tokens(1000)
            .with_max_uses(2)
            .with_use_cache(false)
            .with_response_inclusion(ResponseInclusion::Full);
        assert_eq!(
            serde_json::to_value(&tool).unwrap(),
            json!({
                "name": "web_fetch",
                "blocked_domains": ["bad.example"],
                "citations": {"enabled": true},
                "max_content_tokens": 1000,
                "max_uses": 2,
                "use_cache": false,
                "response_inclusion": "full"
            })
        );
    }

    #[test]
    fn domain_builders_are_exclusive() {
        let tool = WebFetchTool::new()
            .with_blocked_domains(vec!["a".to_string()])
            .with_allowed_domains(vec!["b".to_string()]);
        assert!(tool.blocked_domains.is_none());
        assert!(tool.allowed_domains.is_some());
    }
}
