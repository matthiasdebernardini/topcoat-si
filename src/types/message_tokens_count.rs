use serde::{Deserialize, Serialize};

use crate::types::CountTokensContextManagement;

/// Count of tokens in a message.
#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageTokensCount {
    /// The total number of tokens across the provided list of messages, system prompt,
    /// and tools.
    pub input_tokens: u32,

    /// Context-management information, when the request configured it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_management: Option<CountTokensContextManagement>,
}

impl MessageTokensCount {
    /// Create a new `MessageTokensCount` with the given input tokens.
    pub fn new(input_tokens: u32) -> Self {
        Self {
            input_tokens,
            context_management: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{from_value, json, to_value};

    #[test]
    fn message_tokens_count_serialization() {
        let count = MessageTokensCount::new(123);

        let json = to_value(count).unwrap();
        assert_eq!(
            json,
            json!({
                "input_tokens": 123
            })
        );
    }

    #[test]
    fn message_tokens_count_deserialization() {
        let json = json!({
            "input_tokens": 456
        });

        let count: MessageTokensCount = from_value(json).unwrap();
        assert_eq!(count.input_tokens, 456);
    }

    #[test]
    fn message_tokens_count_with_context_management() {
        let json = json!({
            "input_tokens": 100,
            "context_management": {"original_input_tokens": 5000}
        });

        let count: MessageTokensCount = from_value(json.clone()).unwrap();
        assert_eq!(
            count.context_management,
            Some(CountTokensContextManagement {
                original_input_tokens: 5000
            })
        );
        assert_eq!(to_value(count).unwrap(), json);
    }
}
