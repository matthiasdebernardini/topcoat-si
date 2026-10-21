use std::fmt;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Time-bucket width for a usage report.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
    fn as_str(self) -> &'static str {
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
    /// Filter by service tier: batch, flex, flex_discount, priority, priority_on_demand, standard.
    pub service_tiers: Vec<String>,
    /// Filter by context window: `"0-200k"` or `"200k-1M"`.
    pub context_window: Vec<String>,
    /// Filter by inference geography: global, us, not_available.
    pub inference_geos: Vec<String>,
    /// Filter by speed: standard, fast.
    pub speeds: Vec<String>,
    /// Maximum buckets to return (31 for 1d, 168 for 1h, 1440 for 1m).
    pub limit: Option<u32>,
    /// Pagination cursor from a previous report's `next_page`.
    pub page: Option<String>,
}

impl UsageReportParams {
    /// Create parameters reporting from `starting_at`.
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
    pub fn with_ending_at(mut self, ending_at: OffsetDateTime) -> Self {
        self.ending_at = Some(ending_at);
        self
    }

    /// Set the bucket width.
    pub fn with_bucket_width(mut self, bucket_width: BucketWidth) -> Self {
        self.bucket_width = Some(bucket_width);
        self
    }

    /// Add a grouping dimension.
    pub fn group_by(mut self, group_by: UsageGroupBy) -> Self {
        self.group_by.push(group_by);
        self
    }

    /// Filter by one model.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.models.push(model.into());
        self
    }

    /// Filter by several models.
    pub fn with_models(mut self, models: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.models.extend(models.into_iter().map(Into::into));
        self
    }

    /// Filter by one API key.
    pub fn with_api_key_id(mut self, id: impl Into<String>) -> Self {
        self.api_key_ids.push(id.into());
        self
    }

    /// Filter by several API keys.
    pub fn with_api_key_ids(mut self, ids: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.api_key_ids.extend(ids.into_iter().map(Into::into));
        self
    }

    /// Filter by one workspace.
    pub fn with_workspace_id(mut self, id: impl Into<String>) -> Self {
        self.workspace_ids.push(id.into());
        self
    }

    /// Filter by several workspaces.
    pub fn with_workspace_ids(mut self, ids: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.workspace_ids.extend(ids.into_iter().map(Into::into));
        self
    }

    /// Filter by one service account.
    pub fn with_service_account_id(mut self, id: impl Into<String>) -> Self {
        self.service_account_ids.push(id.into());
        self
    }

    /// Filter by several service accounts.
    pub fn with_service_account_ids(
        mut self,
        ids: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.service_account_ids
            .extend(ids.into_iter().map(Into::into));
        self
    }

    /// Filter by one account.
    pub fn with_account_id(mut self, id: impl Into<String>) -> Self {
        self.account_ids.push(id.into());
        self
    }

    /// Filter by several accounts.
    pub fn with_account_ids(mut self, ids: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.account_ids.extend(ids.into_iter().map(Into::into));
        self
    }

    /// Filter by one service tier.
    pub fn with_service_tier(mut self, tier: impl Into<String>) -> Self {
        self.service_tiers.push(tier.into());
        self
    }

    /// Filter by one context window.
    pub fn with_context_window(mut self, window: impl Into<String>) -> Self {
        self.context_window.push(window.into());
        self
    }

    /// Filter by one inference geography.
    pub fn with_inference_geo(mut self, geo: impl Into<String>) -> Self {
        self.inference_geos.push(geo.into());
        self
    }

    /// Filter by one speed.
    pub fn with_speed(mut self, speed: impl Into<String>) -> Self {
        self.speeds.push(speed.into());
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
        if let Some(width) = self.bucket_width {
            query.append_pair("bucket_width", &width.to_string());
        }
        for group in &self.group_by {
            query.append_pair("group_by[]", group.as_str());
        }
        let arrays: [(&str, &[String]); 8] = [
            ("models[]", &self.models),
            ("api_key_ids[]", &self.api_key_ids),
            ("workspace_ids[]", &self.workspace_ids),
            ("service_account_ids[]", &self.service_account_ids),
            ("account_ids[]", &self.account_ids),
            ("service_tiers[]", &self.service_tiers),
            ("context_window[]", &self.context_window),
            ("inference_geos[]", &self.inference_geos),
        ];
        for (key, values) in arrays {
            for value in values {
                query.append_pair(key, value);
            }
        }
        for speed in &self.speeds {
            query.append_pair("speeds[]", speed);
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

pub(crate) fn rfc3339(time: OffsetDateTime) -> String {
    // Formatting an `OffsetDateTime` as RFC 3339 only fails for years outside 0..=9999.
    time.format(&Rfc3339).unwrap_or_default()
}

/// Cache-creation tokens by cache lifetime in a usage report.
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    pub service_tier: Option<String>,
    /// Inference speed, when grouped by speed.
    #[serde(default)]
    pub speed: Option<String>,
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
    pub fn total_input_tokens(&self) -> i64 {
        self.uncached_input_tokens
            + self.cache_read_input_tokens
            + self.cache_creation.ephemeral_5m_input_tokens
            + self.cache_creation.ephemeral_1h_input_tokens
    }
}

/// One time bucket of a usage report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
        assert_eq!(result.service_tier.as_deref(), Some("standard"));
        assert_eq!(result.speed.as_deref(), Some("standard"));
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
            .with_speed("fast")
            .with_limit(7)
            .with_page("page_abc")
            .to_query();
        assert_eq!(
            query,
            "starting_at=2025-08-01T00%3A00%3A00Z&ending_at=2025-08-08T00%3A00%3A00Z\
             &bucket_width=1d&group_by%5B%5D=model&group_by%5B%5D=workspace_id\
             &models%5B%5D=claude-opus-5-5&speeds%5B%5D=fast&limit=7&page=page_abc"
        );
    }
}
