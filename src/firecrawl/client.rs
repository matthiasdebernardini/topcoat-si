//! The Firecrawl HTTP client.

use std::fmt;
use std::time::Duration;

use reqwest::Client as ReqwestClient;
use reqwest::header::{AUTHORIZATION, HeaderValue};
use reqwest::redirect;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::time::sleep;

use super::{ScrapeRequest, ScrapeResponse, SearchRequest, SearchResponse};
use crate::backoff;
use crate::base_url;
use crate::error::{Error, ErrorCode, HttpMetadata, RequestPhase, Result, map_reqwest_error};
use crate::types::retry_after_from_headers;

const DEFAULT_BASE_URL: &str = "https://api.firecrawl.dev";
const API_KEY_ENV: &str = "FIRECRAWL_API_KEY";
// Scraping each result can take tens of seconds, so allow well beyond Firecrawl's own default.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
// Waiting out a longer Retry-After would stall the conversation, so fail right away instead.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(30);
// Slack between Firecrawl's server-side scrape timeout and the client-side request timeout.
const SCRAPE_TIMEOUT_MARGIN: Duration = Duration::from_secs(30);

/// Client for Firecrawl's search and scrape APIs.
///
/// The API key is only kept as a sensitive header value and is redacted from `Debug`.
#[derive(Clone)]
pub struct FirecrawlClient {
    pub(super) http: ReqwestClient,
    authorization: HeaderValue,
    pub(super) base_url: String,
    max_retries: usize,
}

impl fmt::Debug for FirecrawlClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FirecrawlClient")
            .field("api_key", &"[REDACTED]")
            .field("base_url", &self.base_url)
            .field("max_retries", &self.max_retries)
            .finish_non_exhaustive()
    }
}

impl FirecrawlClient {
    /// Create a client with the given Firecrawl API key.
    ///
    /// Fails when the key is empty after trimming, holds characters not allowed in an HTTP
    /// header, or the HTTP client cannot be built.
    pub fn try_new(api_key: &str) -> Result<Self> {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return Err(Error::validation(
                "Firecrawl API key is empty",
                Some("api_key".to_string()),
            ));
        }
        let mut authorization =
            HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|_| {
                Error::validation(
                    "Firecrawl API key contains characters not allowed in an HTTP header",
                    Some("api_key".to_string()),
                )
            })?;
        authorization.set_sensitive(true);
        Ok(Self {
            http: http_client(DEFAULT_BASE_URL)?,
            authorization,
            base_url: DEFAULT_BASE_URL.to_string(),
            max_retries: 2,
        })
    }

    /// Create a client from the `FIRECRAWL_API_KEY` environment variable.
    pub fn from_env() -> Result<Self> {
        match std::env::var(API_KEY_ENV) {
            Ok(key) if !key.trim().is_empty() => Self::try_new(&key),
            _ => Err(Error::validation(
                format!("{API_KEY_ENV} is not set or empty"),
                Some(API_KEY_ENV.to_string()),
            )),
        }
    }

    /// Send requests to another base URL, such as a local test server.
    ///
    /// The URL must use https; plain http is accepted only for loopback hosts, so the key never
    /// travels in clear. Credentials, a query or a fragment in the URL are rejected. A trailing
    /// slash is trimmed.
    pub fn with_base_url(mut self, base_url: &str) -> Result<Self> {
        self.base_url = base_url::checked_base_url("Firecrawl", "base_url", base_url)?;
        // A loopback base URL bypasses proxies, so the HTTP client depends on it too.
        self.http = http_client(&self.base_url)?;
        Ok(self)
    }

    /// Set how many times a retryable failure is retried. Defaults to 2.
    #[must_use]
    pub fn with_max_retries(mut self, max_retries: usize) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Run a search.
    ///
    /// The request is validated before anything is sent. Responses 408, 429, 500, 502, 503
    /// and 504 and connection failures are retried, honoring `Retry-After`. A
    /// `Retry-After` over 30 seconds fails at once without retrying.
    pub async fn search(&self, request: &SearchRequest) -> Result<SearchResponse> {
        request.validate()?;
        self.post("/v2/search", &request.wire(), None).await
    }

    /// Scrape one page as markdown.
    ///
    /// The request is validated before anything is sent and retried like
    /// [`FirecrawlClient::search`]. A page that loaded with an error status (such as 404) is
    /// still a successful scrape: check
    /// [`ResultMetadata::status_code`](super::ResultMetadata::status_code).
    pub async fn scrape(&self, request: &ScrapeRequest) -> Result<ScrapeResponse> {
        request.validate()?;
        // Firecrawl may work for the whole scrape timeout; give the response time to arrive.
        let timeout = request
            .options
            .timeout
            .map(|t| (t + SCRAPE_TIMEOUT_MARGIN).max(DEFAULT_TIMEOUT));
        self.post("/v2/scrape", &request.wire(), timeout).await
    }

    /// Posts `body` to `path`, retrying transient failures.
    async fn post<B, T>(&self, path: &str, body: &B, timeout: Option<Duration>) -> Result<T>
    where
        B: Serialize + ?Sized,
        T: DeserializeOwned + Succeeded,
    {
        let url = format!("{}{path}", self.base_url);
        let mut attempt = 0;
        loop {
            let error = match self.send_once(&url, body, timeout).await {
                Ok(response) => return Ok(response),
                Err(error) => error,
            };
            let too_long = error.retry_after().is_some_and(|d| d > MAX_RETRY_AFTER);
            if attempt >= self.max_retries || !is_retryable(&error) || too_long {
                return Err(error);
            }
            let delay = error
                .retry_after()
                .unwrap_or_else(|| backoff::next(attempt));
            tracing::warn!(
                target: "topcoat_si::firecrawl",
                error = &error as &dyn std::error::Error,
                path,
                attempt = attempt + 1,
                sleep_seconds = delay.as_secs_f64(),
                "Retrying Firecrawl request"
            );
            sleep(delay).await;
            attempt += 1;
        }
    }

    async fn send_once<B, T>(&self, url: &str, body: &B, timeout: Option<Duration>) -> Result<T>
    where
        B: Serialize + ?Sized,
        T: DeserializeOwned + Succeeded,
    {
        let mut request = self
            .http
            .post(url)
            .header(AUTHORIZATION, self.authorization.clone())
            .json(body);
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        let response = request
            .send()
            .await
            .map_err(|e| map_reqwest_error(e, RequestPhase::Headers))?;
        let status = response.status().as_u16();
        let retry_after = retry_after_from_headers(response.headers());
        let bytes = response
            .bytes()
            .await
            .map_err(|e| map_reqwest_error(e, RequestPhase::Body))?;
        if !(200..300).contains(&status) {
            return Err(http_error(status, &bytes, retry_after));
        }
        let parsed: T = serde_json::from_slice(&bytes).map_err(|e| {
            Error::serialization(
                format!(
                    "Failed to parse Firecrawl response ({} bytes): {e}",
                    bytes.len()
                ),
                Some(Box::new(e)),
            )
        })?;
        if !parsed.succeeded() {
            return Err(http_error(status, &bytes, None));
        }
        Ok(parsed)
    }
}

/// Builds the HTTP client for `base_url`, which decides whether proxies are used (see
/// [`base_url::with_proxy_rule`]).
fn http_client(base_url: &str) -> Result<ReqwestClient> {
    let builder = ReqwestClient::builder()
        .redirect(redirect::Policy::none())
        .connect_timeout(Duration::from_secs(30))
        .timeout(DEFAULT_TIMEOUT);
    base_url::with_proxy_rule(builder, base_url)
        .build()
        .map_err(|e| {
            Error::http_client(
                format!("Failed to build HTTP client: {e}"),
                Some(Box::new(e)),
            )
        })
}

/// A response body carrying Firecrawl's `success` flag.
trait Succeeded {
    fn succeeded(&self) -> bool;
}

impl Succeeded for SearchResponse {
    fn succeeded(&self) -> bool {
        self.success
    }
}

impl Succeeded for ScrapeResponse {
    fn succeeded(&self) -> bool {
        self.success
    }
}

/// Firecrawl's transient statuses (408, 429, 500, 502, 503, 504) and connection failures.
fn is_retryable(error: &Error) -> bool {
    matches!(error, Error::Connection { .. })
        || matches!(error.status_code(), Some(408 | 429 | 500 | 502 | 503 | 504))
}

fn http_error(status: u16, body: &[u8], retry_after: Option<Duration>) -> Error {
    let parsed = serde_json::from_slice::<serde_json::Value>(body).ok();
    let field = |name: &str| {
        parsed
            .as_ref()
            .and_then(|value| value[name].as_str())
            .map(String::from)
    };
    let code = field("code");
    let message = match (field("error"), &code) {
        (Some(error), Some(code)) => format!("Firecrawl returned {status}: {error} (code {code})"),
        (Some(error), None) => format!("Firecrawl returned {status}: {error}"),
        (None, Some(code)) => format!("Firecrawl returned {status} (code {code})"),
        (None, None) => format!("Firecrawl returned {status}"),
    };
    let metadata = HttpMetadata {
        status,
        error_code: code.map(ErrorCode::Firecrawl),
        request_id: None,
        retry_after,
    };
    match status {
        401 => Error::Authentication {
            message,
            metadata,
            source: None,
        },
        429 => Error::RateLimit {
            message,
            metadata,
            source: None,
        },
        408 => Error::Timeout {
            metadata: Some(Box::new(metadata)),
            message,
            source: None,
        },
        _ => Error::Api {
            message,
            metadata,
            source: None,
        },
    }
}
