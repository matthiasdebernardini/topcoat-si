use serde::{Deserialize, Deserializer, Serialize};

use crate::types::{Model, open_union};

/// Additional details returned when generation stops with a refusal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct StopDetails {
    /// Detail discriminator.
    pub r#type: StopDetailsType,
    /// Stable refusal category, when one applies.
    pub category: Option<String>,
    /// Human-readable explanation; callers should display rather than parse it.
    pub explanation: Option<String>,
    /// Model suggested for a direct retry when server-side fallback could not run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_model: Option<Model>,
    /// Opaque token used by the fallback-credit beta.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_credit_token: Option<String>,
    /// Whether the fallback credit permits an appended assistant prefill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_has_prefill_claim: Option<bool>,
}

/// The kind of [`StopDetails`].
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum StopDetailsType {
    /// The model refused; see [`StopDetails::category`].
    Refusal,
    /// A kind this crate does not know.
    Unknown,
}

impl<'de> Deserialize<'de> for StopDetailsType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        open_union::deserialize_str(
            deserializer,
            "StopDetailsType",
            |value| match value {
                "refusal" => Some(Self::Refusal),
                _ => None,
            },
            Self::Unknown,
        )
    }
}
