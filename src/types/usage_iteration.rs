use crate::types::{CacheCreation, Model, open_union};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// Token counts for an individual inference or compaction iteration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct IterationUsage {
    /// Model for this iteration, when supplied by the API.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<Model>,
    /// Input tokens consumed.
    #[serde(default)]
    pub input_tokens: i32,
    /// Output tokens consumed.
    #[serde(default)]
    pub output_tokens: i32,
    /// Tokens written to the cache.
    #[serde(default)]
    pub cache_creation_input_tokens: i32,
    /// Breakdown of the cache writes by cache lifetime, when supplied by the API.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation: Option<CacheCreation>,
    /// Tokens read from the cache.
    #[serde(default)]
    pub cache_read_input_tokens: i32,
}

/// Each known API iteration kind, accepting future kinds without losing the message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum UsageIteration {
    /// Primary inference; model may be absent or null.
    Message(IterationUsage),
    /// Fallback inference.
    FallbackMessage(IterationUsage),
    /// Context compaction.
    Compaction(IterationUsage),
    /// Advisor sub-inference.
    AdvisorMessage(IterationUsage),
    /// An iteration introduced by a newer API.
    Unknown,
}

impl<'de> Deserialize<'de> for UsageIteration {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        match open_union::type_tag::<D::Error>(&value)?.as_str() {
            "message" => open_union::from_value(value).map(Self::Message),
            "fallback_message" => open_union::from_value(value).map(Self::FallbackMessage),
            "compaction" => open_union::from_value(value).map(Self::Compaction),
            "advisor_message" => open_union::from_value(value).map(Self::AdvisorMessage),
            tag => {
                open_union::warn_unknown("UsageIteration", tag);
                Ok(Self::Unknown)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn iteration_round_trips_with_and_without_cache_creation() {
        let with = json!({
            "type": "message",
            "input_tokens": 10,
            "output_tokens": 2,
            "cache_creation_input_tokens": 30,
            "cache_creation": {"ephemeral_5m_input_tokens": 10, "ephemeral_1h_input_tokens": 20},
            "cache_read_input_tokens": 4
        });
        let parsed: UsageIteration = serde_json::from_value(with.clone()).unwrap();
        let UsageIteration::Message(it) = &parsed else {
            panic!("expected a message iteration");
        };
        assert_eq!(it.cache_creation, Some(CacheCreation::new(10, 20)));
        assert_eq!(serde_json::to_value(&parsed).unwrap(), with);

        let without = json!({"type": "compaction", "input_tokens": 5, "output_tokens": 1});
        let parsed: UsageIteration = serde_json::from_value(without).unwrap();
        let UsageIteration::Compaction(it) = &parsed else {
            panic!("expected a compaction iteration");
        };
        assert_eq!(it.cache_creation, None);
        assert_eq!(it.cache_creation_input_tokens, 0);
        assert_eq!(
            serde_json::to_value(&parsed).unwrap(),
            json!({
                "type": "compaction",
                "input_tokens": 5,
                "output_tokens": 1,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0
            })
        );
    }
}
