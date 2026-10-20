use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::types::usage_report::rfc3339;

/// A dimension to group cost-report results by.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostGroupBy {
    /// Group by line-item description (also returns model, token type and tier details).
    Description,
    /// Group by workspace.
    WorkspaceId,
}

impl CostGroupBy {
    fn as_str(self) -> &'static str {
        match self {
            CostGroupBy::Description => "description",
            CostGroupBy::WorkspaceId => "workspace_id",
        }
    }
}

/// Parameters for the Admin API cost report (`GET /v1/organizations/cost_report`).
///
/// The bucket width is always one day.
#[derive(Debug, Clone, PartialEq)]
pub struct CostReportParams {
    /// Start of the reporting window (inclusive).
    pub starting_at: OffsetDateTime,
    /// End of the reporting window (exclusive).
    pub ending_at: Option<OffsetDateTime>,
    /// Dimensions to group results by.
    pub group_by: Vec<CostGroupBy>,
    /// Maximum buckets to return (1 to 31).
    pub limit: Option<u32>,
    /// Pagination cursor from a previous report's `next_page`.
    pub page: Option<String>,
}

impl CostReportParams {
    /// Create parameters reporting from `starting_at`.
    pub fn new(starting_at: OffsetDateTime) -> Self {
        Self {
            starting_at,
            ending_at: None,
            group_by: Vec::new(),
            limit: None,
            page: None,
        }
    }

    /// Set the end of the reporting window.
    pub fn with_ending_at(mut self, ending_at: OffsetDateTime) -> Self {
        self.ending_at = Some(ending_at);
        self
    }

    /// Add a grouping dimension.
    pub fn group_by(mut self, group_by: CostGroupBy) -> Self {
        self.group_by.push(group_by);
        self
    }

    /// Set the maximum number of buckets to return.
    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Set the pagination cursor.
    pub fn with_page(mut self, page: impl Into<String>) -> Self {
        self.page = Some(page.into());
        self
    }

    /// Build the URL-encoded query string, repeating array keys with a `[]` suffix.
    pub fn to_query(&self) -> String {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("starting_at", &rfc3339(self.starting_at));
        if let Some(ending_at) = self.ending_at {
            query.append_pair("ending_at", &rfc3339(ending_at));
        }
        query.append_pair("bucket_width", "1d");
        for group in &self.group_by {
            query.append_pair("group_by[]", group.as_str());
        }
        if let Some(limit) = self.limit {
            query.append_pair("limit", &limit.to_string());
        }
        if let Some(page) = &self.page {
            query.append_pair("page", page);
        }
        query.finish()
    }
}

/// One cost line inside a cost bucket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostBucketResult {
    /// Decimal string in the currency's lowest unit (cents): `"123.78912"` is $1.2378912.
    pub amount: String,
    /// Currency code, for example `"USD"`.
    pub currency: String,
    /// Line-item description, when grouped by description.
    #[serde(default)]
    pub description: Option<String>,
    /// Cost type, for example `"tokens"`.
    #[serde(default)]
    pub cost_type: Option<String>,
    /// Model identifier, when grouped by description.
    #[serde(default)]
    pub model: Option<String>,
    /// Service tier, when grouped by description.
    #[serde(default)]
    pub service_tier: Option<String>,
    /// Inference speed, when grouped by description.
    #[serde(default)]
    pub speed: Option<String>,
    /// Token type, when grouped by description.
    #[serde(default)]
    pub token_type: Option<String>,
    /// Context window, when grouped by description.
    #[serde(default)]
    pub context_window: Option<String>,
    /// Inference geography, when grouped by description.
    #[serde(default)]
    pub inference_geo: Option<String>,
    /// Workspace identifier, when grouped by workspace.
    #[serde(default)]
    pub workspace_id: Option<String>,
}

impl CostBucketResult {
    /// Parse [`amount`](Self::amount) exactly into micro-cents (millionths of a cent).
    ///
    /// The fraction is padded or truncated to 6 digits; no floats are involved.  Returns
    /// `None` for negatives, non-digits, or values that overflow `u64`.
    pub fn amount_micro_cents(&self) -> Option<u64> {
        let (whole, fraction) = self
            .amount
            .split_once('.')
            .unwrap_or((self.amount.as_str(), ""));
        let digits = |text: &str| text.bytes().all(|byte| byte.is_ascii_digit());
        if whole.is_empty() || !digits(whole) || !digits(fraction) {
            return None;
        }
        let mut padded: String = fraction.chars().take(6).collect();
        while padded.len() < 6 {
            padded.push('0');
        }
        whole
            .parse::<u64>()
            .ok()?
            .checked_mul(1_000_000)?
            .checked_add(padded.parse::<u64>().ok()?)
    }
}

/// One daily bucket of a cost report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostBucket {
    /// Start of the bucket (inclusive).
    #[serde(with = "time::serde::rfc3339")]
    pub starting_at: OffsetDateTime,
    /// End of the bucket (exclusive).
    #[serde(with = "time::serde::rfc3339")]
    pub ending_at: OffsetDateTime,
    /// Cost lines in this bucket.
    pub results: Vec<CostBucketResult>,
}

/// A page of the Admin API cost report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostReport {
    /// The buckets on this page.
    pub data: Vec<CostBucket>,
    /// Whether more pages follow.
    pub has_more: bool,
    /// Cursor for the next page, to pass as [`CostReportParams::page`].
    #[serde(default)]
    pub next_page: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::format_description::well_known::Rfc3339;

    const REPORT: &str = r#"{"data":[{"ending_at":"2025-08-02T00:00:00Z","results":[{"amount":"123.78912","context_window":"0-200k","cost_type":"tokens","currency":"USD","description":"Claude Opus 5.5 Usage - Input Tokens","inference_geo":"global","model":"claude-opus-5-5","service_tier":"standard","speed":"standard","token_type":"cache_creation.ephemeral_1h_input_tokens","workspace_id":"wrkspc_01JwQvzr7rXLA5AGx3HKfFUJ"}],"starting_at":"2025-08-01T00:00:00Z"}],"has_more":true,"next_page":"page_MjAyNS0wNS0xNFQwMDowMDowMFo="}"#;

    fn result(amount: &str) -> CostBucketResult {
        CostBucketResult {
            amount: amount.to_string(),
            currency: "USD".to_string(),
            description: None,
            cost_type: None,
            model: None,
            service_tier: None,
            speed: None,
            token_type: None,
            context_window: None,
            inference_geo: None,
            workspace_id: None,
        }
    }

    #[test]
    fn deserializes_docs_example() {
        let report: CostReport = serde_json::from_str(REPORT).unwrap();
        assert!(report.has_more);
        assert_eq!(
            report.next_page.as_deref(),
            Some("page_MjAyNS0wNS0xNFQwMDowMDowMFo=")
        );
        let result = &report.data[0].results[0];
        assert_eq!(result.amount, "123.78912");
        assert_eq!(result.currency, "USD");
        assert_eq!(
            result.description.as_deref(),
            Some("Claude Opus 5.5 Usage - Input Tokens")
        );
        assert_eq!(result.cost_type.as_deref(), Some("tokens"));
        assert_eq!(result.model.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(
            result.token_type.as_deref(),
            Some("cache_creation.ephemeral_1h_input_tokens")
        );
        assert_eq!(result.inference_geo.as_deref(), Some("global"));
        assert_eq!(result.amount_micro_cents(), Some(123_789_120));
    }

    #[test]
    fn amount_micro_cents_is_exact() {
        assert_eq!(result("123.78912").amount_micro_cents(), Some(123_789_120));
        assert_eq!(result("0.5").amount_micro_cents(), Some(500_000));
        assert_eq!(result("7").amount_micro_cents(), Some(7_000_000));
        assert_eq!(result("1.23456789").amount_micro_cents(), Some(1_234_567));
        assert_eq!(result("-1.5").amount_micro_cents(), None);
        assert_eq!(result("1e3").amount_micro_cents(), None);
        assert_eq!(result("").amount_micro_cents(), None);
        assert_eq!(result(".5").amount_micro_cents(), None);
    }

    #[test]
    fn query_uses_bracket_keys_and_rfc3339() {
        let start = OffsetDateTime::parse("2025-08-01T00:00:00Z", &Rfc3339).unwrap();
        let query = CostReportParams::new(start)
            .group_by(CostGroupBy::Description)
            .group_by(CostGroupBy::WorkspaceId)
            .with_limit(31)
            .to_query();
        assert_eq!(
            query,
            "starting_at=2025-08-01T00%3A00%3A00Z&bucket_width=1d\
             &group_by%5B%5D=description&group_by%5B%5D=workspace_id&limit=31"
        );
    }
}
