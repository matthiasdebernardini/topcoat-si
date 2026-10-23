//! The team's credit balance.

use serde::{Deserialize, Serialize};

/// The team's credits, from [`FirecrawlClient::credit_usage`](super::FirecrawlClient::credit_usage).
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CreditUsage {
    /// Credits left for the team.
    pub remaining_credits: i64,
    /// Credits in the plan, not counting coupons or pay-as-you-go top-ups.
    #[serde(default)]
    pub plan_credits: Option<u64>,
    /// Start of the current billing period, as an RFC 3339 timestamp.
    #[serde(default)]
    pub billing_period_start: Option<String>,
    /// End of the current billing period, as an RFC 3339 timestamp.
    #[serde(default)]
    pub billing_period_end: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct CreditUsageResponse {
    pub(super) success: bool,
    #[serde(default)]
    pub(super) data: Option<CreditUsage>,
}
