use serde::{Deserialize, Serialize};

use crate::types::CacheTtl;
use crate::types::wire_tag::EphemeralTag;

/// Marks a prompt-cache breakpoint: the API caches the prompt prefix up to and
/// including the block that carries it.
///
/// The cache entry lives five minutes unless `ttl` is set. "Ephemeral" is the API's
/// name for this cache type; it does not mean "do not cache".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct CacheControlEphemeral {
    /// Cache type, always `"ephemeral"`.
    #[serde(default)]
    r#type: EphemeralTag,

    /// How long the cache breakpoint lives. Omitted means five minutes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl: Option<CacheTtl>,
}

impl CacheControlEphemeral {
    /// Creates a breakpoint with the default five-minute lifetime.
    #[must_use]
    pub fn new() -> Self {
        Self {
            r#type: EphemeralTag,
            ttl: None,
        }
    }

    /// Creates a cache control with a one-hour lifetime.
    #[must_use]
    pub fn one_hour() -> Self {
        Self::new().with_ttl(CacheTtl::OneHour)
    }

    /// Sets the cache lifetime.
    #[must_use]
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

        assert_eq!(cache_control.r#type, EphemeralTag);
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
