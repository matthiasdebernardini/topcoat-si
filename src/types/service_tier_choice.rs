use serde::{Deserialize, Serialize};

/// Which service tier a request may use.
///
/// This is the request-side choice; the tier that actually served a request is
/// reported by [`ServiceTier`](crate::types::ServiceTier).
#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ServiceTierChoice {
    /// Use priority capacity when available and fall back to the standard tier.
    Auto,
    /// Only use the standard tier.
    StandardOnly,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serialization() {
        assert_eq!(
            serde_json::to_value(ServiceTierChoice::Auto).unwrap(),
            json!("auto")
        );
        assert_eq!(
            serde_json::to_value(ServiceTierChoice::StandardOnly).unwrap(),
            json!("standard_only")
        );
    }

    #[test]
    fn deserialization() {
        let choice: ServiceTierChoice = serde_json::from_value(json!("standard_only")).unwrap();
        assert_eq!(choice, ServiceTierChoice::StandardOnly);
    }
}
