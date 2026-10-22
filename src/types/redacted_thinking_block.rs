use serde::{Deserialize, Serialize};

/// A redacted thinking block that contains encoded/obscured thinking data.
///
/// This block is used when the full thinking contents are not directly accessible.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct RedactedThinkingBlock {
    /// The encoded thinking data (redacted from normal display).
    pub data: String,
}

impl RedactedThinkingBlock {
    /// Creates a new RedactedThinkingBlock with the specified data.
    #[must_use]
    pub fn new(data: impl Into<String>) -> Self {
        Self { data: data.into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

    #[test]
    fn redacted_thinking_block_serialization() {
        let block = RedactedThinkingBlock::new("encoded-thinking-data-123");

        let json = serde_json::to_string(&block).unwrap();
        let expected = r#"{"data":"encoded-thinking-data-123"}"#;

        assert_eq!(json, expected);
    }

    #[test]
    fn deserialization() {
        let json = r#"{"data":"encoded-thinking-data-123","type":"redacted_thinking"}"#;
        let block: RedactedThinkingBlock = serde_json::from_str(json).unwrap();

        assert_eq!(block.data, "encoded-thinking-data-123");
    }

    #[test]
    fn new_with_str() {
        let block = RedactedThinkingBlock::new("Redacted thinking content");
        let json = to_value(&block).unwrap();

        assert_eq!(
            json,
            json!({
                "data": "Redacted thinking content"
            })
        );
    }
}
