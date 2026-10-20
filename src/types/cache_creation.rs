use std::ops::Add;

use serde::{Deserialize, Serialize};

/// Breakdown of cache-creation input tokens by cache lifetime.
#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CacheCreation {
    /// The number of input tokens used to create the 5-minute cache entry.
    pub ephemeral_5m_input_tokens: i32,

    /// The number of input tokens used to create the 1-hour cache entry.
    pub ephemeral_1h_input_tokens: i32,
}

impl CacheCreation {
    /// Create a new `CacheCreation` with the given 5-minute and 1-hour token counts.
    pub fn new(ephemeral_5m_input_tokens: i32, ephemeral_1h_input_tokens: i32) -> Self {
        Self {
            ephemeral_5m_input_tokens,
            ephemeral_1h_input_tokens,
        }
    }
}

impl Add for CacheCreation {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self {
            ephemeral_5m_input_tokens: self.ephemeral_5m_input_tokens
                + rhs.ephemeral_5m_input_tokens,
            ephemeral_1h_input_tokens: self.ephemeral_1h_input_tokens
                + rhs.ephemeral_1h_input_tokens,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

    #[test]
    fn cache_creation_round_trip() {
        let value = to_value(CacheCreation::new(2000, 3000)).unwrap();
        assert_eq!(
            value,
            json!({"ephemeral_5m_input_tokens": 2000, "ephemeral_1h_input_tokens": 3000})
        );
        let parsed: CacheCreation = serde_json::from_value(value).unwrap();
        assert_eq!(parsed, CacheCreation::new(2000, 3000));
    }

    #[test]
    fn add_cache_creation() {
        let sum = CacheCreation::new(1, 2) + CacheCreation::new(10, 20);
        assert_eq!(sum, CacheCreation::new(11, 22));
    }
}
