use serde::{Deserialize, Deserializer, Serialize};

use crate::types::open_union;

/// Lifecycle state of a model in the Models API.
#[derive(Debug, Copy, Clone, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ModelLifecycle {
    /// The model is generally available.
    Active,
    /// The model is deprecated and has a retirement date.
    Deprecated,
    /// The model has been retired.
    Retired,
    /// A lifecycle state this crate does not know.
    ///
    /// Never send it; it only models values this crate does not know.
    Unknown,
}

impl<'de> Deserialize<'de> for ModelLifecycle {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        open_union::deserialize_str(
            deserializer,
            "ModelLifecycle",
            |value| match value {
                "active" => Some(Self::Active),
                "deprecated" => Some(Self::Deprecated),
                "retired" => Some(Self::Retired),
                _ => None,
            },
            Self::Unknown,
        )
    }
}

impl ModelLifecycle {
    /// The wire value used in query strings and responses.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            ModelLifecycle::Active => "active",
            ModelLifecycle::Deprecated => "deprecated",
            ModelLifecycle::Retired => "retired",
            ModelLifecycle::Unknown => "unknown",
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
