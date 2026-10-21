use serde::{Deserialize, Serialize};

/// Configuration for enabling or disabling citations in the response.
///
/// This type allows controlling whether citations will be included in the model's response.
/// By default, citations are disabled.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct CitationsConfig {
    /// Whether citations are enabled
    pub enabled: bool,
}

impl CitationsConfig {
    /// Returns a CitationsConfig with citations enabled
    #[must_use]
    pub fn enabled() -> Self {
        Self { enabled: true }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialization_enabled() {
        let config = CitationsConfig::enabled();
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json, serde_json::json!({"enabled": true}));
    }

    #[test]
    fn serialization_disabled() {
        let config = CitationsConfig::default();
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json, serde_json::json!({"enabled": false}));
    }

    #[test]
    fn deserialization() {
        let json = serde_json::json!({"enabled": true});
        let config: CitationsConfig = serde_json::from_value(json).unwrap();
        assert!(config.enabled);

        let json = serde_json::json!({"enabled": false});
        let config: CitationsConfig = serde_json::from_value(json).unwrap();
        assert!(!config.enabled);
    }
}
