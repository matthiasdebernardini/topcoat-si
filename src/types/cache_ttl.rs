use serde::{Deserialize, Serialize};

/// How long a prompt-cache breakpoint stays alive.
///
/// Omitting the TTL on a cache control means five minutes.
#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CacheTtl {
    /// Five-minute cache lifetime (the API default).
    #[serde(rename = "5m")]
    FiveMinutes,
    /// One-hour cache lifetime.
    #[serde(rename = "1h")]
    OneHour,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serialization() {
        assert_eq!(
            serde_json::to_value(CacheTtl::FiveMinutes).unwrap(),
            json!("5m")
        );
        assert_eq!(
            serde_json::to_value(CacheTtl::OneHour).unwrap(),
            json!("1h")
        );
    }

    #[test]
    fn deserialization() {
        let ttl: CacheTtl = serde_json::from_value(json!("1h")).unwrap();
        assert_eq!(ttl, CacheTtl::OneHour);
        let ttl: CacheTtl = serde_json::from_value(json!("5m")).unwrap();
        assert_eq!(ttl, CacheTtl::FiveMinutes);
    }
}
