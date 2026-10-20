use serde::{Deserialize, Serialize};

/// Fallback-credit outcome reported in `usage` on the fallback-credit beta.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FallbackCredit {
    /// Whether the credit was applied, and why not when it was not.
    pub status: FallbackCreditStatus,
}

impl FallbackCredit {
    /// Create a new `FallbackCredit` with the given status.
    pub fn new(status: FallbackCreditStatus) -> Self {
        Self { status }
    }
}

/// Whether the fallback credit was applied, tagged by `type` on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
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
    #[serde(other)]
    Unknown,
}

/// Why a fallback credit was not applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
    #[serde(other)]
    Unknown,
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
