use std::time::Duration;

use reqwest::header::HeaderMap;
use time::OffsetDateTime;
use time::format_description::well_known::{Rfc2822, Rfc3339};

use crate::http::header_str;

/// One rate-limit window reported by `anthropic-ratelimit-*` response headers.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct RateLimitWindow {
    /// The maximum value permitted in the window.
    pub limit: u64,
    /// The value remaining in the window.
    pub remaining: u64,
    /// When the window fully replenishes, if reported.
    pub reset: Option<OffsetDateTime>,
}

/// Rate-limit state read from the headers of one API response.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct RateLimits {
    /// Requests window.
    pub requests: Option<RateLimitWindow>,
    /// Combined tokens window.
    pub tokens: Option<RateLimitWindow>,
    /// Input tokens window.
    pub input_tokens: Option<RateLimitWindow>,
    /// Output tokens window.
    pub output_tokens: Option<RateLimitWindow>,
    /// Priority Tier input tokens window.
    pub priority_input_tokens: Option<RateLimitWindow>,
    /// Priority Tier output tokens window.
    pub priority_output_tokens: Option<RateLimitWindow>,
    /// Fast-mode input tokens window.
    pub fast_input_tokens: Option<RateLimitWindow>,
    /// Fast-mode output tokens window.
    pub fast_output_tokens: Option<RateLimitWindow>,
    /// How long to wait before retrying (`retry-after-ms` or `retry-after`).
    pub retry_after: Option<Duration>,
    /// The workspace the request ran in (`anthropic-workspace-id`).
    pub workspace_id: Option<String>,
    /// The request identifier (`request-id`).
    pub request_id: Option<String>,
}

fn window(headers: &HeaderMap, prefix: &str) -> Option<RateLimitWindow> {
    let number = |suffix: &str| {
        header_str(headers, &format!("{prefix}-{suffix}"))
            .and_then(|value| value.trim().parse::<u64>().ok())
    };
    Some(RateLimitWindow {
        limit: number("limit")?,
        remaining: number("remaining")?,
        reset: header_str(headers, &format!("{prefix}-reset"))
            .and_then(|value| OffsetDateTime::parse(value.trim(), &Rfc3339).ok()),
    })
}

/// Parse `retry-after-ms`, then `retry-after` as seconds or an HTTP date.
pub(crate) fn retry_after_from_headers(headers: &HeaderMap) -> Option<Duration> {
    let seconds = |name| {
        header_str(headers, name)
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0)
    };
    seconds("retry-after-ms")
        .and_then(|ms| Duration::try_from_secs_f64(ms / 1000.0).ok())
        .or_else(|| seconds("retry-after").and_then(|s| Duration::try_from_secs_f64(s).ok()))
        .or_else(|| {
            header_str(headers, "retry-after")
                .and_then(|value| OffsetDateTime::parse(value, &Rfc2822).ok())
                .map(|deadline| {
                    Duration::from_secs_f64(
                        (deadline - OffsetDateTime::now_utc())
                            .as_seconds_f64()
                            .max(0.0),
                    )
                })
        })
}

impl RateLimits {
    /// Read rate-limit state from response headers.
    ///
    /// A window is `Some` only when both its `-limit` and `-remaining` headers parse.
    /// Malformed values are ignored.
    #[must_use]
    pub fn from_headers(headers: &HeaderMap) -> Self {
        Self {
            requests: window(headers, "anthropic-ratelimit-requests"),
            tokens: window(headers, "anthropic-ratelimit-tokens"),
            input_tokens: window(headers, "anthropic-ratelimit-input-tokens"),
            output_tokens: window(headers, "anthropic-ratelimit-output-tokens"),
            priority_input_tokens: window(headers, "anthropic-priority-input-tokens"),
            priority_output_tokens: window(headers, "anthropic-priority-output-tokens"),
            fast_input_tokens: window(headers, "anthropic-fast-input-tokens"),
            fast_output_tokens: window(headers, "anthropic-fast-output-tokens"),
            retry_after: retry_after_from_headers(headers),
            workspace_id: header_str(headers, "anthropic-workspace-id").map(String::from),
            request_id: header_str(headers, "request-id").map(String::from),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_static(value));
        }
        map
    }

    #[test]
    fn full_header_set_parses() {
        let mut pairs = vec![
            ("retry-after", "30"),
            ("anthropic-workspace-id", "wrkspc_1"),
            ("request-id", "req_1"),
        ];
        pairs.extend([
            ("anthropic-ratelimit-requests-limit", "50"),
            ("anthropic-ratelimit-requests-remaining", "49"),
            ("anthropic-ratelimit-requests-reset", "2026-10-19T12:00:00Z"),
            ("anthropic-ratelimit-tokens-limit", "100"),
            ("anthropic-ratelimit-tokens-remaining", "90"),
            ("anthropic-ratelimit-tokens-reset", "2026-10-19T12:00:01Z"),
            ("anthropic-ratelimit-input-tokens-limit", "60"),
            ("anthropic-ratelimit-input-tokens-remaining", "50"),
            (
                "anthropic-ratelimit-input-tokens-reset",
                "2026-10-19T12:00:02Z",
            ),
            ("anthropic-ratelimit-output-tokens-limit", "40"),
            ("anthropic-ratelimit-output-tokens-remaining", "30"),
            (
                "anthropic-ratelimit-output-tokens-reset",
                "2026-10-19T12:00:03Z",
            ),
            ("anthropic-priority-input-tokens-limit", "6"),
            ("anthropic-priority-input-tokens-remaining", "5"),
            (
                "anthropic-priority-input-tokens-reset",
                "2026-10-19T12:00:04Z",
            ),
            ("anthropic-priority-output-tokens-limit", "4"),
            ("anthropic-priority-output-tokens-remaining", "3"),
            (
                "anthropic-priority-output-tokens-reset",
                "2026-10-19T12:00:05Z",
            ),
            ("anthropic-fast-input-tokens-limit", "8"),
            ("anthropic-fast-input-tokens-remaining", "7"),
            ("anthropic-fast-input-tokens-reset", "2026-10-19T12:00:06Z"),
            ("anthropic-fast-output-tokens-limit", "2"),
            ("anthropic-fast-output-tokens-remaining", "1"),
            ("anthropic-fast-output-tokens-reset", "2026-10-19T12:00:07Z"),
        ]);
        let limits = RateLimits::from_headers(&headers(&pairs));

        let requests = limits.requests.unwrap();
        assert_eq!((requests.limit, requests.remaining), (50, 49));
        assert_eq!(
            requests.reset,
            Some(OffsetDateTime::parse("2026-10-19T12:00:00Z", &Rfc3339).unwrap())
        );
        assert_eq!(limits.tokens.unwrap().remaining, 90);
        assert_eq!(limits.input_tokens.unwrap().limit, 60);
        assert_eq!(limits.output_tokens.unwrap().remaining, 30);
        assert_eq!(limits.priority_input_tokens.unwrap().limit, 6);
        assert_eq!(limits.priority_output_tokens.unwrap().remaining, 3);
        assert_eq!(limits.fast_input_tokens.unwrap().remaining, 7);
        assert_eq!(limits.fast_output_tokens.unwrap().limit, 2);
        assert_eq!(limits.retry_after, Some(Duration::from_secs(30)));
        assert_eq!(limits.workspace_id.as_deref(), Some("wrkspc_1"));
        assert_eq!(limits.request_id.as_deref(), Some("req_1"));
    }

    #[test]
    fn partial_headers_yield_none_windows() {
        let limits = RateLimits::from_headers(&headers(&[
            ("anthropic-ratelimit-requests-limit", "50"),
            ("anthropic-ratelimit-tokens-remaining", "9"),
            ("anthropic-ratelimit-input-tokens-limit", "5"),
            ("anthropic-ratelimit-input-tokens-remaining", "4"),
        ]));
        assert_eq!(limits.requests, None);
        assert_eq!(limits.tokens, None);
        let input = limits.input_tokens.unwrap();
        assert_eq!((input.limit, input.remaining, input.reset), (5, 4, None));
        assert_eq!(limits.retry_after, None);
        assert_eq!(limits.request_id, None);
    }

    #[test]
    fn bad_numbers_and_dates_are_ignored() {
        let limits = RateLimits::from_headers(&headers(&[
            ("anthropic-ratelimit-requests-limit", "many"),
            ("anthropic-ratelimit-requests-remaining", "3"),
            ("anthropic-ratelimit-tokens-limit", "10"),
            ("anthropic-ratelimit-tokens-remaining", "-1"),
            ("anthropic-ratelimit-input-tokens-limit", "10"),
            ("anthropic-ratelimit-input-tokens-remaining", "2"),
            ("anthropic-ratelimit-input-tokens-reset", "soon"),
            ("retry-after", "later"),
        ]));
        assert_eq!(limits.requests, None);
        assert_eq!(limits.tokens, None);
        let input = limits.input_tokens.unwrap();
        assert_eq!((input.limit, input.remaining, input.reset), (10, 2, None));
        assert_eq!(limits.retry_after, None);
    }

    #[test]
    fn retry_after_ms_wins() {
        let limits = RateLimits::from_headers(&headers(&[
            ("retry-after-ms", "1500"),
            ("retry-after", "30"),
        ]));
        assert_eq!(limits.retry_after, Some(Duration::from_millis(1500)));
    }
}
