use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::types::open_union;

/// Fallback-credit outcome reported in `usage` on the fallback-credit beta.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct FallbackCredit {
    /// Whether the credit was applied, and why not when it was not.
    pub status: FallbackCreditStatus,
}

impl FallbackCredit {
    /// Create a new `FallbackCredit` with the given status.
    #[must_use]
    pub fn new(status: FallbackCreditStatus) -> Self {
        Self { status }
    }
}

/// Whether the fallback credit was applied, tagged by `type` on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum FallbackCreditStatus {
    /// The credit was redeemed.
    Redeemed,
    /// The credit was not applied.
    NotApplied {
        /// Why the credit was not applied.
        reason: FallbackCreditReason,
        /// Request fields to remove to redeem the credit, when the API says.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remove_to_redeem: Option<Vec<String>>,
    },
    /// A status this version does not know.
    Unknown,
}

impl<'de> Deserialize<'de> for FallbackCreditStatus {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct NotApplied {
            reason: FallbackCreditReason,
            #[serde(default)]
            remove_to_redeem: Option<Vec<String>>,
        }

        let value = Value::deserialize(deserializer)?;
        match open_union::type_tag::<D::Error>(&value)?.as_str() {
            "redeemed" => Ok(Self::Redeemed),
            "not_applied" => {
                open_union::from_value(value).map(|not_applied: NotApplied| Self::NotApplied {
                    reason: not_applied.reason,
                    remove_to_redeem: not_applied.remove_to_redeem,
                })
            }
            tag => {
                open_union::warn_unknown("FallbackCreditStatus", tag);
                Ok(Self::Unknown)
            }
        }
    }
}

/// Why a fallback credit was not applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum FallbackCreditReason {
    /// The request body does not match the one the credit was issued for.
    BodyMismatch,
    /// The request is a continuation, which the credit excludes.
    ContinuationExcluded,
    /// The credit only applies to continuations.
    ContinuationOnly,
    /// The credit has expired.
    Expired,
    /// The target model is not valid for the credit.
    InvalidTargetModel,
    /// Fallback credits are not enabled.
    NotEnabled,
    /// The request could not be repriced.
    RepriceUnavailable,
    /// Fallback credits are temporarily unavailable.
    TemporarilyUnavailable,
    /// The request carries fields that vary from the original.
    VariantFieldsPresent,
    /// The credit belongs to another organization.
    WrongOrganization,
    /// The credit was issued on another platform.
    WrongPlatform,
    /// The credit belongs to another workspace.
    WrongWorkspace,
    /// A reason this version does not know.
    Unknown,
}

impl<'de> Deserialize<'de> for FallbackCreditReason {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        open_union::deserialize_str(
            deserializer,
            "FallbackCreditReason",
            |value| match value {
                "body_mismatch" => Some(Self::BodyMismatch),
                "continuation_excluded" => Some(Self::ContinuationExcluded),
                "continuation_only" => Some(Self::ContinuationOnly),
                "expired" => Some(Self::Expired),
                "invalid_target_model" => Some(Self::InvalidTargetModel),
                "not_enabled" => Some(Self::NotEnabled),
                "reprice_unavailable" => Some(Self::RepriceUnavailable),
                "temporarily_unavailable" => Some(Self::TemporarilyUnavailable),
                "variant_fields_present" => Some(Self::VariantFieldsPresent),
                "wrong_organization" => Some(Self::WrongOrganization),
                "wrong_platform" => Some(Self::WrongPlatform),
                "wrong_workspace" => Some(Self::WrongWorkspace),
                _ => None,
            },
            Self::Unknown,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redeemed_round_trips() {
        let value = json!({"status": {"type": "redeemed"}});
        let credit: FallbackCredit = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(credit, FallbackCredit::new(FallbackCreditStatus::Redeemed));
        assert_eq!(serde_json::to_value(&credit).unwrap(), value);
    }

    #[test]
    fn not_applied_round_trips() {
        let value = json!({"status": {
            "type": "not_applied",
            "reason": "variant_fields_present",
            "remove_to_redeem": ["tools"]
        }});
        let credit: FallbackCredit = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            credit.status,
            FallbackCreditStatus::NotApplied {
                reason: FallbackCreditReason::VariantFieldsPresent,
                remove_to_redeem: Some(vec!["tools".to_string()]),
            }
        );
        assert_eq!(serde_json::to_value(&credit).unwrap(), value);
    }

    #[test]
    fn unknown_reason_and_status_deserialize() {
        let credit: FallbackCredit = serde_json::from_value(
            json!({"status": {"type": "not_applied", "reason": "solar_flare"}}),
        )
        .unwrap();
        assert_eq!(
            credit.status,
            FallbackCreditStatus::NotApplied {
                reason: FallbackCreditReason::Unknown,
                remove_to_redeem: None,
            }
        );
        assert_eq!(
            serde_json::to_value(&credit).unwrap(),
            json!({"status": {"type": "not_applied", "reason": "unknown"}})
        );

        let credit: FallbackCredit =
            serde_json::from_value(json!({"status": {"type": "postponed"}})).unwrap();
        assert_eq!(credit.status, FallbackCreditStatus::Unknown);
        assert_eq!(
            serde_json::to_value(&credit).unwrap(),
            json!({"status": {"type": "unknown"}})
        );
    }
}
