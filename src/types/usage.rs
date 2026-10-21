use std::ops::Add;

use serde::{Deserialize, Serialize};

use crate::types::{
    CacheCreation, FallbackCredit, InferenceSpeed, OutputTokensDetails, ServerToolUsage,
    ServiceTier, UsageIteration,
};

/// Usage information for API calls.
///
/// Anthropic's API bills and rate-limits by token counts, as tokens represent the
/// underlying cost to their systems.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct Usage {
    /// The number of input tokens used to create the cache entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<i32>,

    /// Breakdown of cache-creation input tokens by cache lifetime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation: Option<CacheCreation>,

    /// The number of input tokens read from the cache.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<i32>,

    /// The number of input tokens which were used.
    pub input_tokens: i32,

    /// The number of output tokens which were used.
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

    /// The service tier that served the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<ServiceTier>,

    /// The geography where inference ran (for example `"global"` or `"us"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inference_geo: Option<String>,

    /// The inference speed the request ran at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<InferenceSpeed>,

    /// Fallback-credit outcome; present on the fallback-credit beta.  Its `status` is tagged
    /// `"redeemed"` or `"not_applied"`.  Boxed to keep `Usage`, and the stream events and
    /// batch results that embed it, small.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_credit: Option<Box<FallbackCredit>>,
}

impl Usage {
    /// Create a new `Usage` with the given input and output tokens.
    #[must_use]
    pub fn new(input_tokens: i32, output_tokens: i32) -> Self {
        Self {
            cache_creation_input_tokens: None,
            cache_creation: None,
            cache_read_input_tokens: None,
            input_tokens,
            output_tokens,
            output_tokens_details: None,
            server_tool_use: None,
            iterations: None,
            service_tier: None,
            inference_geo: None,
            speed: None,
            fallback_credit: None,
        }
    }

    /// Set the cache creation input tokens.
    #[must_use]
    pub fn with_cache_creation_input_tokens(mut self, tokens: i32) -> Self {
        self.cache_creation_input_tokens = Some(tokens);
        self
    }

    /// Set the cache-creation breakdown by cache lifetime.
    #[must_use]
    pub fn with_cache_creation(mut self, cache_creation: CacheCreation) -> Self {
        self.cache_creation = Some(cache_creation);
        self
    }

    /// Set the cache read input tokens.
    #[must_use]
    pub fn with_cache_read_input_tokens(mut self, tokens: i32) -> Self {
        self.cache_read_input_tokens = Some(tokens);
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

    /// Set the service tier.
    #[must_use]
    pub fn with_service_tier(mut self, service_tier: ServiceTier) -> Self {
        self.service_tier = Some(service_tier);
        self
    }

    /// Set the inference geography.
    #[must_use]
    pub fn with_inference_geo(mut self, inference_geo: impl Into<String>) -> Self {
        self.inference_geo = Some(inference_geo.into());
        self
    }

    /// Set the inference speed.
    #[must_use]
    pub fn with_speed(mut self, speed: InferenceSpeed) -> Self {
        self.speed = Some(speed);
        self
    }

    /// Set the fallback-credit outcome.
    #[must_use]
    pub fn with_fallback_credit(mut self, fallback_credit: FallbackCredit) -> Self {
        self.fallback_credit = Some(Box::new(fallback_credit));
        self
    }

    /// Total prompt tokens: input plus cache writes plus cache reads, each clamped at 0.
    ///
    /// This is the count Anthropic uses to pick Claude Haiku 5.5's pricing tier.
    /// It describes a single request; summed usage across requests mis-tiers Claude Haiku 5.5.
    #[must_use]
    pub fn prompt_tokens(&self) -> u64 {
        let clamp = |tokens: i32| u64::try_from(tokens).unwrap_or(0);
        clamp(self.input_tokens)
            + clamp(self.cache_creation_input_tokens.unwrap_or(0))
            + clamp(self.cache_read_input_tokens.unwrap_or(0))
    }
}

/// Helper function to add two Option values where the contained type implements Add.
fn add_options<T: Add<Output = T>>(left: Option<T>, right: Option<T>) -> Option<T> {
    match (left, right) {
        (Some(a), Some(b)) => Some(a + b),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// Adopt a value when only one side has it and keep it when both agree; a merged usage
/// spanning different values has none.
fn adopt_or_drop_conflict<T: PartialEq>(left: Option<T>, right: Option<T>) -> Option<T> {
    match (left, right) {
        (Some(left), Some(right)) => (left == right).then_some(left),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

impl Add for Usage {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self {
            cache_creation_input_tokens: add_options(
                self.cache_creation_input_tokens,
                rhs.cache_creation_input_tokens,
            ),
            cache_creation: add_options(self.cache_creation, rhs.cache_creation),
            cache_read_input_tokens: add_options(
                self.cache_read_input_tokens,
                rhs.cache_read_input_tokens,
            ),
            input_tokens: self.input_tokens + rhs.input_tokens,
            output_tokens: self.output_tokens + rhs.output_tokens,
            output_tokens_details: add_options(
                self.output_tokens_details,
                rhs.output_tokens_details,
            ),
            server_tool_use: add_options(self.server_tool_use, rhs.server_tool_use),
            iterations: match (self.iterations, rhs.iterations) {
                (Some(mut left), Some(right)) => {
                    left.extend(right);
                    Some(left)
                }
                (Some(iterations), None) | (None, Some(iterations)) => Some(iterations),
                (None, None) => None,
            },
            service_tier: adopt_or_drop_conflict(self.service_tier, rhs.service_tier),
            inference_geo: adopt_or_drop_conflict(self.inference_geo, rhs.inference_geo),
            speed: adopt_or_drop_conflict(self.speed, rhs.speed),
            fallback_credit: adopt_or_drop_conflict(self.fallback_credit, rhs.fallback_credit),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FallbackCreditStatus;
    use serde_json::{json, to_value};

    #[test]
    fn usage_minimal() {
        let usage = Usage::new(50, 100);
        let json = to_value(usage).unwrap();

        assert_eq!(
            json,
            json!({
                "input_tokens": 50,
                "output_tokens": 100
            })
        );
    }

    #[test]
    fn usage_complete() {
        let server_tool_use = ServerToolUsage::new(5);
        let usage = Usage::new(50, 100)
            .with_cache_creation_input_tokens(20)
            .with_cache_read_input_tokens(30)
            .with_thinking_tokens(40)
            .with_server_tool_use(server_tool_use);

        let json = to_value(usage).unwrap();

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
    fn usage_cache_creation_and_prompt_tokens() {
        let usage = Usage::new(10, 5)
            .with_cache_creation_input_tokens(5000)
            .with_cache_creation(CacheCreation::new(2000, 3000))
            .with_cache_read_input_tokens(400);
        assert_eq!(usage.prompt_tokens(), 5410);

        let json = to_value(&usage).unwrap();
        assert_eq!(json["cache_creation"]["ephemeral_1h_input_tokens"], 3000);
        let parsed: Usage = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, usage);

        let sum = usage.clone() + usage;
        assert_eq!(sum.cache_creation, Some(CacheCreation::new(4000, 6000)));

        assert_eq!(Usage::new(-3, 1).prompt_tokens(), 0);
    }

    #[test]
    fn usage_deserialization() {
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

        let usage: Usage = serde_json::from_value(json).unwrap();
        assert_eq!(usage.cache_creation_input_tokens, Some(20));
        assert_eq!(usage.cache_read_input_tokens, Some(30));
        assert_eq!(usage.input_tokens, 50);
        assert_eq!(usage.output_tokens, 100);
        assert_eq!(
            usage.output_tokens_details,
            Some(OutputTokensDetails::new(40))
        );
        assert_eq!(usage.server_tool_use, Some(ServerToolUsage::new(5)));
    }

    #[test]
    fn usage_speed_deserializes_known_and_unknown() {
        let usage: Usage = serde_json::from_value(
            json!({"input_tokens": 1, "output_tokens": 2, "speed": "fast", "service_tier": "priority", "inference_geo": "us"}),
        )
        .unwrap();
        assert_eq!(usage.speed, Some(InferenceSpeed::Fast));
        assert_eq!(usage.service_tier, Some(ServiceTier::Priority));
        assert_eq!(usage.inference_geo.as_deref(), Some("us"));

        let usage: Usage = serde_json::from_value(
            json!({"input_tokens": 1, "output_tokens": 2, "speed": "turbo"}),
        )
        .unwrap();
        assert_eq!(usage.speed, Some(InferenceSpeed::Unknown));
    }

    #[test]
    fn add_usage_keeps_equal_tier_speed_geo_and_drops_differing() {
        let a = Usage::new(1, 1)
            .with_service_tier(ServiceTier::Standard)
            .with_speed(InferenceSpeed::Fast)
            .with_inference_geo("us");
        let b = Usage::new(1, 1)
            .with_service_tier(ServiceTier::Standard)
            .with_speed(InferenceSpeed::Standard)
            .with_inference_geo("us");
        let sum = a + b;
        assert_eq!(sum.service_tier, Some(ServiceTier::Standard));
        assert_eq!(sum.speed, None);
        assert_eq!(sum.inference_geo.as_deref(), Some("us"));

        let sum = Usage::new(1, 1).with_service_tier(ServiceTier::Batch) + Usage::new(1, 1);
        assert_eq!(sum.service_tier, Some(ServiceTier::Batch));

        let seeded = Usage::new(0, 0)
            + Usage::new(1, 1)
                .with_service_tier(ServiceTier::Priority)
                .with_speed(InferenceSpeed::Fast)
                .with_inference_geo("us");
        assert_eq!(seeded.service_tier, Some(ServiceTier::Priority));
        assert_eq!(seeded.speed, Some(InferenceSpeed::Fast));
        assert_eq!(seeded.inference_geo.as_deref(), Some("us"));

        let sum = Usage::new(1, 1).with_service_tier(ServiceTier::Batch)
            + Usage::new(1, 1).with_service_tier(ServiceTier::Standard);
        assert_eq!(sum.service_tier, None);
    }

    #[test]
    fn fallback_credit_deserializes_and_add_keeps_equal() {
        let usage: Usage = serde_json::from_value(json!({
            "input_tokens": 1,
            "output_tokens": 2,
            "fallback_credit": {"status": {"type": "redeemed"}}
        }))
        .unwrap();
        assert_eq!(
            usage.fallback_credit,
            Some(Box::new(FallbackCredit::new(
                FallbackCreditStatus::Redeemed
            )))
        );
        assert_eq!(
            to_value(&usage).unwrap()["fallback_credit"]["status"]["type"],
            "redeemed"
        );

        let sum = usage.clone() + usage.clone();
        assert_eq!(
            sum.fallback_credit,
            Some(Box::new(FallbackCredit::new(
                FallbackCreditStatus::Redeemed
            )))
        );
        let sum = usage.clone() + Usage::new(1, 1);
        assert_eq!(sum.fallback_credit, usage.fallback_credit);
        let sum = usage
            + Usage::new(1, 1)
                .with_fallback_credit(FallbackCredit::new(FallbackCreditStatus::Unknown));
        assert_eq!(sum.fallback_credit, None);
    }

    #[test]
    fn add_usage_minimal() {
        let usage1 = Usage::new(50, 100);
        let usage2 = Usage::new(30, 60);
        let result = usage1 + usage2;

        assert_eq!(result.input_tokens, 80);
        assert_eq!(result.output_tokens, 160);
        assert_eq!(result.output_tokens_details, None);
        assert_eq!(result.cache_creation_input_tokens, None);
        assert_eq!(result.cache_read_input_tokens, None);
        assert_eq!(result.server_tool_use, None);
    }

    #[test]
    fn add_usage_with_cache_tokens() {
        let usage1 = Usage::new(50, 100)
            .with_cache_creation_input_tokens(20)
            .with_cache_read_input_tokens(30);
        let usage2 = Usage::new(30, 60)
            .with_cache_creation_input_tokens(10)
            .with_cache_read_input_tokens(15);
        let result = usage1 + usage2;

        assert_eq!(result.input_tokens, 80);
        assert_eq!(result.output_tokens, 160);
        assert_eq!(result.cache_creation_input_tokens, Some(30));
        assert_eq!(result.cache_read_input_tokens, Some(45));
    }

    #[test]
    fn add_usage_partial_options() {
        let usage1 = Usage::new(50, 100).with_cache_creation_input_tokens(20);
        let usage2 = Usage::new(30, 60).with_cache_read_input_tokens(15);
        let result = usage1 + usage2;

        assert_eq!(result.input_tokens, 80);
        assert_eq!(result.output_tokens, 160);
        assert_eq!(result.cache_creation_input_tokens, Some(20));
        assert_eq!(result.cache_read_input_tokens, Some(15));
    }

    #[test]
    fn add_usage_with_server_tool_use() {
        let usage1 = Usage::new(50, 100).with_server_tool_use(ServerToolUsage::new(5));
        let usage2 = Usage::new(30, 60).with_server_tool_use(ServerToolUsage::new(3));
        let result = usage1 + usage2;

        assert_eq!(result.input_tokens, 80);
        assert_eq!(result.output_tokens, 160);
        assert_eq!(result.server_tool_use, Some(ServerToolUsage::new(8)));
    }

    #[test]
    fn add_usage_with_output_tokens_details() {
        let usage1 = Usage::new(50, 100).with_thinking_tokens(40);
        let usage2 = Usage::new(30, 60).with_thinking_tokens(20);
        let result = usage1 + usage2;

        assert_eq!(
            result.output_tokens_details,
            Some(OutputTokensDetails::new(60))
        );
    }

    #[test]
    fn add_usage_complete() {
        let usage1 = Usage::new(50, 100)
            .with_cache_creation_input_tokens(20)
            .with_cache_read_input_tokens(30)
            .with_server_tool_use(ServerToolUsage::new(5));
        let usage2 = Usage::new(30, 60)
            .with_cache_creation_input_tokens(10)
            .with_cache_read_input_tokens(15)
            .with_server_tool_use(ServerToolUsage::new(3));
        let result = usage1 + usage2;

        assert_eq!(result.input_tokens, 80);
        assert_eq!(result.output_tokens, 160);
        assert_eq!(result.cache_creation_input_tokens, Some(30));
        assert_eq!(result.cache_read_input_tokens, Some(45));
        assert_eq!(result.server_tool_use, Some(ServerToolUsage::new(8)));
    }
}
