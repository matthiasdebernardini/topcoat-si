use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::types::{InferenceSpeed, open_union};

/// Time-bucket width for a usage report.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum BucketWidth {
    /// One-minute buckets.
    #[serde(rename = "1m")]
    OneMinute,
    /// One-hour buckets.
    #[serde(rename = "1h")]
    OneHour,
    /// One-day buckets.
    #[serde(rename = "1d")]
    OneDay,
}

impl fmt::Display for BucketWidth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BucketWidth::OneMinute => "1m",
            BucketWidth::OneHour => "1h",
            BucketWidth::OneDay => "1d",
        })
    }
}

/// A dimension to group usage-report results by.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum UsageGroupBy {
    /// Group by account.
    AccountId,
    /// Group by API key.
    ApiKeyId,
    /// Group by context window size.
    ContextWindow,
    /// Group by inference geography.
    InferenceGeo,
    /// Group by model.
    Model,
    /// Group by service account.
    ServiceAccountId,
    /// Group by service tier.
    ServiceTier,
    /// Group by inference speed.
    Speed,
    /// Group by workspace.
    WorkspaceId,
}

impl UsageGroupBy {
    fn as_str(&self) -> &'static str {
        match self {
            UsageGroupBy::AccountId => "account_id",
            UsageGroupBy::ApiKeyId => "api_key_id",
            UsageGroupBy::ContextWindow => "context_window",
            UsageGroupBy::InferenceGeo => "inference_geo",
            UsageGroupBy::Model => "model",
            UsageGroupBy::ServiceAccountId => "service_account_id",
            UsageGroupBy::ServiceTier => "service_tier",
            UsageGroupBy::Speed => "speed",
            UsageGroupBy::WorkspaceId => "workspace_id",
        }
    }
}

/// Parameters for the Admin API usage report (`GET /v1/organizations/usage_report/messages`).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct UsageReportParams {
    /// Start of the reporting window (inclusive).
    pub starting_at: OffsetDateTime,
    /// End of the reporting window (exclusive).
    pub ending_at: Option<OffsetDateTime>,
    /// Bucket width; the API defaults to one day.
    pub bucket_width: Option<BucketWidth>,
    /// Dimensions to group results by.
    pub group_by: Vec<UsageGroupBy>,
    /// Filter by model identifiers.
    pub models: Vec<String>,
    /// Filter by API key identifiers.
    pub api_key_ids: Vec<String>,
    /// Filter by workspace identifiers.
    pub workspace_ids: Vec<String>,
    /// Filter by service account identifiers.
    pub service_account_ids: Vec<String>,
    /// Filter by account identifiers.
    pub account_ids: Vec<String>,
    /// Filter by service tier.
    pub service_tiers: Vec<UsageReportServiceTier>,
    /// Filter by context window: `"0-200k"` or `"200k-1M"`.
    pub context_window: Vec<String>,
    /// Filter by inference geography: global, us, not_available.
    pub inference_geos: Vec<String>,
    /// Filter by speed.
    pub speeds: Vec<InferenceSpeed>,
    /// Maximum buckets to return (31 for 1d, 168 for 1h, 1440 for 1m).
    pub limit: Option<u32>,
    /// Pagination cursor from a previous report's `next_page`.
    pub page: Option<String>,
}

impl UsageReportParams {
    /// Create parameters reporting from `starting_at`.
    #[must_use]
    pub fn new(starting_at: OffsetDateTime) -> Self {
        Self {
            starting_at,
            ending_at: None,
            bucket_width: None,
            group_by: Vec::new(),
            models: Vec::new(),
            api_key_ids: Vec::new(),
            workspace_ids: Vec::new(),
            service_account_ids: Vec::new(),
            account_ids: Vec::new(),
            service_tiers: Vec::new(),
            context_window: Vec::new(),
            inference_geos: Vec::new(),
            speeds: Vec::new(),
            limit: None,
            page: None,
        }
    }

    /// Set the end of the reporting window.
    #[must_use]
    pub fn with_ending_at(mut self, ending_at: OffsetDateTime) -> Self {
        self.ending_at = Some(ending_at);
        self
    }

    /// Set the bucket width.
    #[must_use]
    pub fn with_bucket_width(mut self, bucket_width: BucketWidth) -> Self {
        self.bucket_width = Some(bucket_width);
        self
    }

    /// Add a grouping dimension.
    #[must_use]
    pub fn group_by(mut self, group_by: UsageGroupBy) -> Self {
        self.group_by.push(group_by);
        self
    }

    /// Filter by one model.
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.models.push(model.into());
        self
    }

    /// Filter by one API key.
    #[must_use]
    pub fn with_api_key_id(mut self, id: impl Into<String>) -> Self {
        self.api_key_ids.push(id.into());
        self
    }

    /// Filter by one workspace.
    #[must_use]
    pub fn with_workspace_id(mut self, id: impl Into<String>) -> Self {
        self.workspace_ids.push(id.into());
        self
    }

    /// Filter by one service account.
    #[must_use]
    pub fn with_service_account_id(mut self, id: impl Into<String>) -> Self {
        self.service_account_ids.push(id.into());
        self
    }

    /// Filter by one account.
    #[must_use]
    pub fn with_account_id(mut self, id: impl Into<String>) -> Self {
        self.account_ids.push(id.into());
        self
    }

    /// Filter by one service tier.
    #[must_use]
    pub fn with_service_tier(mut self, tier: UsageReportServiceTier) -> Self {
        self.service_tiers.push(tier);
        self
    }

    /// Filter by one context window.
    #[must_use]
    pub fn with_context_window(mut self, window: impl Into<String>) -> Self {
        self.context_window.push(window.into());
        self
    }

    /// Filter by one inference geography.
    #[must_use]
    pub fn with_inference_geo(mut self, geo: impl Into<String>) -> Self {
        self.inference_geos.push(geo.into());
        self
    }

    /// Filter by one speed.
    #[must_use]
    pub fn with_speed(mut self, speed: InferenceSpeed) -> Self {
        self.speeds.push(speed);
        self
    }

    /// Set the maximum number of buckets to return.
    #[must_use]
    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Set the pagination cursor.
    #[must_use]
    pub fn with_page(mut self, page: impl Into<String>) -> Self {
        self.page = Some(page.into());
        self
    }

    /// Rejects an `Unknown` service tier or speed read from a reply, which has no API value to
    /// send.
    pub(crate) fn check_known_values(&self) -> crate::Result<()> {
        if let Some(index) = self
            .service_tiers
            .iter()
            .position(|tier| *tier == UsageReportServiceTier::Unknown)
        {
            return Err(open_union::unknown_in_request(format!(
                "service_tiers[{index}]"
            )));
        }
        self.speeds
            .iter()
            .position(|speed| *speed == InferenceSpeed::Unknown)
            .map_or(Ok(()), |index| {
                Err(open_union::unknown_in_request(format!("speeds[{index}]")))
            })
    }

    /// The query-string pairs for this request, repeating array keys with a `[]` suffix.
    pub(crate) fn query_pairs(&self) -> Vec<(String, String)> {
        let mut pairs = Vec::new();
        pairs.push(("starting_at".to_string(), rfc3339(self.starting_at)));
        if let Some(ending_at) = self.ending_at {
            pairs.push(("ending_at".to_string(), rfc3339(ending_at)));
        }
        if let Some(width) = self.bucket_width {
            pairs.push(("bucket_width".to_string(), width.to_string()));
        }
        for group in &self.group_by {
            pairs.push(("group_by[]".to_string(), group.as_str().to_string()));
        }
        let arrays: [(&str, &[String]); 7] = [
            ("models[]", &self.models),
            ("api_key_ids[]", &self.api_key_ids),
            ("workspace_ids[]", &self.workspace_ids),
            ("service_account_ids[]", &self.service_account_ids),
            ("account_ids[]", &self.account_ids),
            ("context_window[]", &self.context_window),
            ("inference_geos[]", &self.inference_geos),
        ];
        for (key, values) in arrays {
            for value in values {
                pairs.push((key.to_string(), value.to_string()));
            }
        }
        for tier in &self.service_tiers {
            pairs.push(("service_tiers[]".to_string(), wire_str(tier)));
        }
        for speed in &self.speeds {
            pairs.push(("speeds[]".to_string(), wire_str(speed)));
        }
        if let Some(limit) = self.limit {
            pairs.push(("limit".to_string(), limit.to_string()));
        }
        if let Some(page) = &self.page {
            pairs.push(("page".to_string(), page.clone()));
        }
        pairs
    }
}

/// The string a string enum serializes to; a unit variant always serializes to a string.
fn wire_str<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => text,
        _ => String::new(),
    }
}

pub(crate) fn rfc3339(time: OffsetDateTime) -> String {
    // Formatting an `OffsetDateTime` as RFC 3339 only fails for years outside 0..=9999.
    time.format(&Rfc3339).unwrap_or_default()
}

/// Cache-creation tokens by cache lifetime in a usage report.
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct UsageReportCacheCreation {
    /// Tokens written to the 1-hour cache.
    #[serde(default)]
    pub ephemeral_1h_input_tokens: i64,
    /// Tokens written to the 5-minute cache.
    #[serde(default)]
    pub ephemeral_5m_input_tokens: i64,
}

/// One result row inside a usage bucket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct UsageBucketResult {
    /// Input tokens that were not served from or written to the cache.
    pub uncached_input_tokens: i64,
    /// Output tokens.
    pub output_tokens: i64,
    /// Input tokens read from the cache.
    pub cache_read_input_tokens: i64,
    /// Cache-creation tokens by lifetime.
    #[serde(default)]
    pub cache_creation: UsageReportCacheCreation,
    /// Model identifier, when grouped by model.
    #[serde(default)]
    pub model: Option<String>,
    /// Service tier, when grouped by service tier.
    #[serde(default)]
    pub service_tier: Option<UsageReportServiceTier>,
    /// Inference speed, when grouped by speed.
    #[serde(default)]
    pub speed: Option<InferenceSpeed>,
    /// Context window, when grouped by context window.
    #[serde(default)]
    pub context_window: Option<String>,
    /// Inference geography, when grouped by it.
    #[serde(default)]
    pub inference_geo: Option<String>,
    /// API key identifier, when grouped by API key.
    #[serde(default)]
    pub api_key_id: Option<String>,
    /// Workspace identifier, when grouped by workspace.
    #[serde(default)]
    pub workspace_id: Option<String>,
    /// Account identifier, when grouped by account.
    #[serde(default)]
    pub account_id: Option<String>,
    /// Service account identifier, when grouped by service account.
    #[serde(default)]
    pub service_account_id: Option<String>,
}

impl UsageBucketResult {
    /// Total input tokens: uncached plus cache reads plus 5-minute and 1-hour cache writes.
    #[must_use]
    pub fn total_input_tokens(&self) -> i64 {
        self.uncached_input_tokens
            + self.cache_read_input_tokens
            + self.cache_creation.ephemeral_5m_input_tokens
            + self.cache_creation.ephemeral_1h_input_tokens
    }
}

/// The service tier of a usage-report row.
///
/// The report knows more tiers than a Messages reply's [`ServiceTier`](crate::types::ServiceTier).
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum UsageReportServiceTier {
    /// Standard capacity.
    Standard,
    /// Priority Tier capacity.
    Priority,
    /// Priority capacity bought on demand.
    PriorityOnDemand,
    /// Message Batches processing.
    Batch,
    /// Flex capacity.
    Flex,
    /// Flex capacity at the discounted rate.
    FlexDiscount,
    /// A tier this crate does not know.
    Unknown,
}

impl<'de> Deserialize<'de> for UsageReportServiceTier {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        open_union::deserialize_str(
            deserializer,
            "UsageReportServiceTier",
            |value| match value {
                "standard" => Some(Self::Standard),
                "priority" => Some(Self::Priority),
                "priority_on_demand" => Some(Self::PriorityOnDemand),
                "batch" => Some(Self::Batch),
                "flex" => Some(Self::Flex),
                "flex_discount" => Some(Self::FlexDiscount),
                _ => None,
            },
            Self::Unknown,
        )
    }
}

/// One time bucket of a usage report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct UsageBucket {
    /// Start of the bucket (inclusive).
    #[serde(with = "time::serde::rfc3339")]
    pub starting_at: OffsetDateTime,
    /// End of the bucket (exclusive).
    #[serde(with = "time::serde::rfc3339")]
    pub ending_at: OffsetDateTime,
    /// Result rows in this bucket.
    pub results: Vec<UsageBucketResult>,
}

/// A page of the Admin API usage report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct UsageReport {
    /// The buckets on this page.
    pub data: Vec<UsageBucket>,
    /// Whether more pages follow.
    pub has_more: bool,
    /// Cursor for the next page, to pass as [`UsageReportParams::page`].
    #[serde(default)]
    pub next_page: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::encode_query;

    const REPORT: &str = r#"{"data":[{"ending_at":"2025-08-02T00:00:00Z","results":[{"account_id":"user_01WCz1FkmYMm4gnmykNKUu3Q","api_key_id":"apikey_01Rj2N8SVvo6BePZj99NhmiT","cache_creation":{"ephemeral_1h_input_tokens":0,"ephemeral_5m_input_tokens":0},"cache_read_input_tokens":200,"context_window":"0-200k","inference_geo":"global","model":"claude-opus-5-5","output_tokens":500,"server_tool_use":{"web_search_requests":10},"service_account_id":"svac_01Hk3R9TWxq7CfQak00OiVw4","service_tier":"standard","speed":"standard","uncached_input_tokens":1500,"workspace_id":"wrkspc_01JwQvzr7rXLA5AGx3HKfFUJ"}],"starting_at":"2025-08-01T00:00:00Z"}],"has_more":true,"next_page":"page_MjAyNS0wNS0xNFQwMDowMDowMFo="}"#;

    #[test]
    fn deserializes_docs_example() {
        let report: UsageReport = serde_json::from_str(REPORT).unwrap();
        assert!(report.has_more);
        assert_eq!(
            report.next_page.as_deref(),
            Some("page_MjAyNS0wNS0xNFQwMDowMDowMFo=")
        );
        let bucket = &report.data[0];
        assert_eq!(bucket.starting_at.year(), 2025);
        assert_eq!(bucket.ending_at.day(), 2);
        let result = &bucket.results[0];
        assert_eq!(result.uncached_input_tokens, 1500);
        assert_eq!(result.output_tokens, 500);
        assert_eq!(result.cache_read_input_tokens, 200);
        assert_eq!(result.cache_creation, UsageReportCacheCreation::default());
        assert_eq!(result.model.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(result.service_tier, Some(UsageReportServiceTier::Standard));
        assert_eq!(result.speed, Some(InferenceSpeed::Standard));
        assert_eq!(result.context_window.as_deref(), Some("0-200k"));
        assert_eq!(result.inference_geo.as_deref(), Some("global"));
        assert_eq!(
            result.workspace_id.as_deref(),
            Some("wrkspc_01JwQvzr7rXLA5AGx3HKfFUJ")
        );
        assert_eq!(
            result.service_account_id.as_deref(),
            Some("svac_01Hk3R9TWxq7CfQak00OiVw4")
        );
        assert_eq!(result.total_input_tokens(), 1700);
    }

    #[test]
    fn query_uses_bracket_keys_and_rfc3339() {
        let start = OffsetDateTime::parse("2025-08-01T00:00:00Z", &Rfc3339).unwrap();
        let end = OffsetDateTime::parse("2025-08-08T00:00:00Z", &Rfc3339).unwrap();
        let query = UsageReportParams::new(start)
            .with_ending_at(end)
            .with_bucket_width(BucketWidth::OneDay)
            .group_by(UsageGroupBy::Model)
            .group_by(UsageGroupBy::WorkspaceId)
            .with_model("claude-opus-5-5")
            .with_speed(InferenceSpeed::Fast)
            .with_limit(7)
            .with_page("page_abc")
            .query_pairs();
        assert_eq!(
            encode_query(query),
            "starting_at=2025-08-01T00%3A00%3A00Z&ending_at=2025-08-08T00%3A00%3A00Z\
             &bucket_width=1d&group_by%5B%5D=model&group_by%5B%5D=workspace_id\
             &models%5B%5D=claude-opus-5-5&speeds%5B%5D=fast&limit=7&page=page_abc"
        );
    }

    #[test]
    fn unknown_result_speed_parses_and_warns_once() {
        let row = serde_json::json!({
            "uncached_input_tokens": 1,
            "output_tokens": 2,
            "cache_read_input_tokens": 0,
            "speed": "warp"
        });
        let (result, warnings) = crate::test_support::unknown_warnings(|| {
            serde_json::from_value::<UsageBucketResult>(row).unwrap()
        });
        assert_eq!(result.speed, Some(InferenceSpeed::Unknown));
        assert_eq!(warnings, ["rust_type=InferenceSpeed tag=warp"]);
    }

    #[test]
    fn typed_filters_encode_their_wire_values_and_reject_unknown() {
        let start = OffsetDateTime::parse("2025-08-01T00:00:00Z", &Rfc3339).unwrap();
        let params = UsageReportParams::new(start)
            .with_service_tier(UsageReportServiceTier::PriorityOnDemand)
            .with_service_tier(UsageReportServiceTier::FlexDiscount)
            .with_speed(InferenceSpeed::Standard);
        assert!(params.check_known_values().is_ok());
        assert_eq!(
            encode_query(params.query_pairs()),
            "starting_at=2025-08-01T00%3A00%3A00Z&service_tiers%5B%5D=priority_on_demand\
             &service_tiers%5B%5D=flex_discount&speeds%5B%5D=standard"
        );

        let param = |params: UsageReportParams| match params.check_known_values() {
            Err(error) => match error.kind() {
                crate::ErrorKind::Validation { param } => param.clone(),
                other => panic!("expected a validation error, got {other:?}"),
            },
            Ok(()) => panic!("expected a validation error"),
        };
        let tier = params
            .clone()
            .with_service_tier(UsageReportServiceTier::Unknown);
        assert_eq!(param(tier).as_deref(), Some("service_tiers[2]"));
        let speed = params.with_speed(InferenceSpeed::Unknown);
        assert_eq!(param(speed).as_deref(), Some("speeds[1]"));
    }
}
