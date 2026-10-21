use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};

use crate::types::open_union;

/// The service tier that served a request, as reported in `usage.service_tier`.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ServiceTier {
    /// Standard capacity.
    Standard,
    /// Priority Tier capacity.
    Priority,
    /// Message Batches processing, billed at 50% of standard rates.
    Batch,
    /// A tier this crate does not know.
    Unknown,
}

impl<'de> Deserialize<'de> for ServiceTier {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        open_union::deserialize_str(
            deserializer,
            "ServiceTier",
            |value| match value {
                "standard" => Some(Self::Standard),
                "priority" => Some(Self::Priority),
                "batch" => Some(Self::Batch),
                _ => None,
            },
            Self::Unknown,
        )
    }
}

impl fmt::Display for ServiceTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServiceTier::Standard => write!(f, "standard"),
            ServiceTier::Priority => write!(f, "priority"),
            ServiceTier::Batch => write!(f, "batch"),
            ServiceTier::Unknown => write!(f, "unknown"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_tier_round_trip_and_unknown() {
        let tier: ServiceTier = serde_json::from_str("\"batch\"").unwrap();
        assert_eq!(tier, ServiceTier::Batch);
        assert_eq!(
            serde_json::to_string(&ServiceTier::Priority).unwrap(),
            "\"priority\""
        );
        let tier: ServiceTier = serde_json::from_str("\"flex\"").unwrap();
        assert_eq!(tier, ServiceTier::Unknown);
        assert_eq!(ServiceTier::Standard.to_string(), "standard");
    }
}
