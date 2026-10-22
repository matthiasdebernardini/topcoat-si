use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::types::thinking_block;

/// A thinking delta, representing a piece of thinking in a streaming response.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct ThinkingDelta {
    /// The thinking content.
    pub thinking: String,
}

impl<'de> Deserialize<'de> for ThinkingDelta {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        Ok(Self {
            thinking: thinking_block::thinking_text(
                &value,
                &["thinking", "summary", "text", "content"],
                "",
            )
            .unwrap_or_default(),
        })
    }
}

impl ThinkingDelta {
    /// Create a new `ThinkingDelta` with the given thinking text.
    #[must_use]
    pub fn new(thinking: impl Into<String>) -> Self {
        Self {
            thinking: thinking.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

    #[test]
    fn thinking_delta_serialization() {
        let delta = ThinkingDelta::new("Let me think about this...");
        let json = to_value(&delta).unwrap();

        assert_eq!(
            json,
            json!({
                "thinking": "Let me think about this..."
            })
        );
    }

    #[test]
    fn thinking_delta_deserialization() {
        let json = json!({
            "thinking": "Let me think about this..."
        });

        let delta: ThinkingDelta = serde_json::from_value(json).unwrap();
        assert_eq!(delta.thinking, "Let me think about this...");
    }

    #[test]
    fn thinking_delta_deserialization_accepts_summary() {
        let json = json!({
            "summary": "Summarized thinking..."
        });

        let delta: ThinkingDelta = serde_json::from_value(json).unwrap();
        assert_eq!(delta.thinking, "Summarized thinking...");
    }

    #[test]
    fn thinking_delta_deserialization_accepts_text() {
        let json = json!({
            "text": "Text-shaped thinking..."
        });

        let delta: ThinkingDelta = serde_json::from_value(json).unwrap();
        assert_eq!(delta.thinking, "Text-shaped thinking...");
    }
}
