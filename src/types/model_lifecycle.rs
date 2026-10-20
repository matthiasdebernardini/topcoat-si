use serde::{Deserialize, Serialize};

/// Lifecycle state of a model in the Models API.
#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ModelLifecycle {
    /// The model is generally available.
    Active,
    /// The model is deprecated and has a retirement date.
    Deprecated,
    /// The model has been retired.
    Retired,
}

impl ModelLifecycle {
    /// The wire value used in query strings and responses.
    pub fn as_str(self) -> &'static str {
        match self {
            ModelLifecycle::Active => "active",
            ModelLifecycle::Deprecated => "deprecated",
            ModelLifecycle::Retired => "retired",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serialization_matches_as_str() {
        for lifecycle in [
            ModelLifecycle::Active,
            ModelLifecycle::Deprecated,
            ModelLifecycle::Retired,
        ] {
            assert_eq!(
                serde_json::to_value(lifecycle).unwrap(),
                json!(lifecycle.as_str())
            );
        }
    }

    #[test]
    fn deserialization() {
        let lifecycle: ModelLifecycle = serde_json::from_value(json!("deprecated")).unwrap();
        assert_eq!(lifecycle, ModelLifecycle::Deprecated);
    }
}
