use serde::{Deserialize, Serialize};

use crate::types::CacheTtl;

/// CacheControlEphemeral specifies that content should be ephemeral, meaning it should
/// not be cached or persisted beyond the immediate request.
///
/// This is useful for sensitive information that shouldn't be stored long-term.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CacheControlEphemeral {
    /// The type is always "ephemeral" for this struct
    #[serde(default = "default_type")]
    pub r#type: String,

    /// How long the cache breakpoint lives. Omitted means five minutes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl: Option<CacheTtl>,
}

fn default_type() -> String {
    "ephemeral".to_string()
}

impl CacheControlEphemeral {
    /// Creates a new CacheControlEphemeral instance
    pub fn new() -> Self {
        Self {
            r#type: default_type(),
            ttl: None,
        }
    }

    /// Creates a cache control with a one-hour lifetime.
    pub fn one_hour() -> Self {
        Self::new().with_ttl(CacheTtl::OneHour)
    }

    /// Sets the cache lifetime.
    pub fn with_ttl(mut self, ttl: CacheTtl) -> Self {
        self.ttl = Some(ttl);
        self
    }
}

impl Default for CacheControlEphemeral {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialization() {
        let cache_control = CacheControlEphemeral::new();

        let json = serde_json::to_value(&cache_control).unwrap();
        let expected = serde_json::json!({"type": "ephemeral"});

        assert_eq!(json, expected);
    }

    #[test]
    fn deserialization() {
        let json = serde_json::json!({"type": "ephemeral"});
        let cache_control: CacheControlEphemeral = serde_json::from_value(json).unwrap();

        assert_eq!(cache_control.r#type, "ephemeral");
        assert_eq!(cache_control.ttl, None);
    }

    #[test]
    fn one_hour_serialization() {
        let json = serde_json::to_value(CacheControlEphemeral::one_hour()).unwrap();
        assert_eq!(json, serde_json::json!({"type": "ephemeral", "ttl": "1h"}));
    }

    #[test]
    fn ttl_deserialization() {
        let json = serde_json::json!({"type": "ephemeral", "ttl": "5m"});
        let cache_control: CacheControlEphemeral = serde_json::from_value(json).unwrap();
        assert_eq!(cache_control.ttl, Some(CacheTtl::FiveMinutes));
    }
}
