use serde::{Deserialize, Serialize};

use crate::types::{OutputTokensDetails, ServerToolUsage, Usage, UsageIteration};

/// Message delta usage information.
///
/// Anthropic's API bills and rate-limits by token counts, as tokens represent the
/// underlying cost to their systems.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct MessageDeltaUsage {
    /// The cumulative number of input tokens used to create the cache entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<i32>,

    /// The cumulative number of input tokens read from the cache.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<i32>,

    /// The cumulative number of input tokens which were used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<i32>,

    /// The cumulative number of output tokens which were used.
    pub output_tokens: i32,

    /// Breakdown of output tokens by category.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens_details: Option<OutputTokensDetails>,

    /// The number of server tool requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_tool_use: Option<ServerToolUsage>,

    /// Per-model attempts made by fallback routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iterations: Option<Vec<UsageIteration>>,
}

impl MessageDeltaUsage {
    /// Create a new `MessageDeltaUsage` with the given output tokens.
    #[must_use]
    pub fn new(output_tokens: i32) -> Self {
        Self {
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
            input_tokens: None,
            output_tokens,
            output_tokens_details: None,
            server_tool_use: None,
            iterations: None,
        }
    }

    /// Set the cache creation input tokens.
    #[must_use]
    pub fn with_cache_creation_input_tokens(mut self, tokens: i32) -> Self {
        self.cache_creation_input_tokens = Some(tokens);
        self
    }

    /// Set the cache read input tokens.
    #[must_use]
    pub fn with_cache_read_input_tokens(mut self, tokens: i32) -> Self {
        self.cache_read_input_tokens = Some(tokens);
        self
    }

    /// Set the input tokens.
    #[must_use]
    pub fn with_input_tokens(mut self, tokens: i32) -> Self {
        self.input_tokens = Some(tokens);
        self
    }

    /// Set the output-token breakdown.
    #[must_use]
    pub fn with_output_tokens_details(mut self, details: OutputTokensDetails) -> Self {
        self.output_tokens_details = Some(details);
        self
    }

    /// Set the thinking-token count in the output-token breakdown.
    #[must_use]
    pub fn with_thinking_tokens(self, thinking_tokens: i32) -> Self {
        self.with_output_tokens_details(OutputTokensDetails::new(thinking_tokens))
    }

    /// Set the server tool usage.
    #[must_use]
    pub fn with_server_tool_use(mut self, server_tool_use: ServerToolUsage) -> Self {
        self.server_tool_use = Some(server_tool_use);
        self
    }

    /// Merge this delta into the usage accumulated from `message_start`.
    ///
    /// Every `Some` field overrides the accumulated value; `None` keeps it.  Fields the delta
    /// does not carry (`service_tier`, `inference_geo`, `speed`, `cache_creation`) stay as
    /// received on `message_start`.
    pub fn merge_into(&self, usage: &mut Usage) {
        if let Some(input_tokens) = self.input_tokens {
            usage.input_tokens = input_tokens;
        }
        usage.output_tokens = self.output_tokens;
        if let Some(tokens) = self.cache_creation_input_tokens {
            usage.cache_creation_input_tokens = Some(tokens);
        }
        if let Some(tokens) = self.cache_read_input_tokens {
            usage.cache_read_input_tokens = Some(tokens);
        }
        if let Some(details) = self.output_tokens_details {
            usage.output_tokens_details = Some(details);
        }
        if let Some(server_tool_use) = self.server_tool_use {
            usage.server_tool_use = Some(server_tool_use);
        }
        if let Some(iterations) = &self.iterations {
            usage.iterations = Some(iterations.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CacheCreation, InferenceSpeed, ServiceTier};
    use serde_json::{json, to_value};

    #[test]
    fn message_delta_usage_minimal() {
        let usage = MessageDeltaUsage::new(100);
        let json = to_value(&usage).unwrap();

        assert_eq!(
            json,
            json!({
                "output_tokens": 100
            })
        );
    }

    #[test]
    fn message_delta_usage_complete() {
        let server_tool_use = ServerToolUsage::new(5);
        let usage = MessageDeltaUsage::new(100)
            .with_cache_creation_input_tokens(20)
            .with_cache_read_input_tokens(30)
            .with_input_tokens(50)
            .with_thinking_tokens(40)
            .with_server_tool_use(server_tool_use);

        let json = to_value(&usage).unwrap();

        assert_eq!(
            json,
            json!({
                "cache_creation_input_tokens": 20,
                "cache_read_input_tokens": 30,
                "input_tokens": 50,
                "output_tokens": 100,
                "output_tokens_details": {
                    "thinking_tokens": 40
                },
                "server_tool_use": {
                    "web_fetch_requests": 5
                }
            })
        );
    }

    #[test]
    fn merge_into_overrides_only_carried_fields() {
        let mut usage = Usage::new(100, 1)
            .with_cache_read_input_tokens(7)
            .with_cache_creation(CacheCreation::new(1, 2))
            .with_service_tier(ServiceTier::Standard)
            .with_inference_geo("global")
            .with_speed(InferenceSpeed::Standard);
        let delta = MessageDeltaUsage::new(50)
            .with_cache_creation_input_tokens(3)
            .with_thinking_tokens(9);
        delta.merge_into(&mut usage);

        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cache_creation_input_tokens, Some(3));
        assert_eq!(usage.cache_read_input_tokens, Some(7));
        assert_eq!(
            usage.output_tokens_details,
            Some(OutputTokensDetails::new(9))
        );
        assert_eq!(usage.service_tier, Some(ServiceTier::Standard));
        assert_eq!(usage.inference_geo.as_deref(), Some("global"));
        assert_eq!(usage.speed, Some(InferenceSpeed::Standard));
        assert_eq!(usage.cache_creation, Some(CacheCreation::new(1, 2)));
    }

    #[test]
    fn delta_ignores_service_tier_inference_geo_speed_and_cache_creation() {
        let delta: MessageDeltaUsage = serde_json::from_value(json!({
            "output_tokens": 5,
            "service_tier": "batch",
            "inference_geo": "us",
            "speed": "fast",
            "cache_creation": {"ephemeral_5m_input_tokens": 1, "ephemeral_1h_input_tokens": 2}
        }))
        .unwrap();
        let mut usage = Usage::new(1, 1);
        delta.merge_into(&mut usage);
        assert_eq!(usage.service_tier, None);
        assert_eq!(usage.inference_geo, None);
        assert_eq!(usage.speed, None);
        assert_eq!(usage.cache_creation, None);
    }

    #[test]
    fn message_delta_usage_deserialization() {
        let json = json!({
            "cache_creation_input_tokens": 20,
            "cache_read_input_tokens": 30,
            "input_tokens": 50,
            "output_tokens": 100,
            "output_tokens_details": {
                "thinking_tokens": 40
            },
            "server_tool_use": {
                "web_search_requests": 3,
                "web_fetch_requests": 5
            }
        });

        let usage: MessageDeltaUsage = serde_json::from_value(json).unwrap();
        assert_eq!(usage.cache_creation_input_tokens, Some(20));
        assert_eq!(usage.cache_read_input_tokens, Some(30));
        assert_eq!(usage.input_tokens, Some(50));
        assert_eq!(usage.output_tokens, 100);
        assert_eq!(
            usage.output_tokens_details,
            Some(OutputTokensDetails::new(40))
        );
        assert_eq!(usage.server_tool_use, Some(ServerToolUsage::new(5)));
    }
}
