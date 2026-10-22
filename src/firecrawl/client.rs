//! The Firecrawl HTTP client.

use std::fmt;
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, HeaderValue};
use reqwest::{Client as ReqwestClient, ClientBuilder};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{ScrapeRequest, ScrapeResponse, SearchRequest, SearchResponse};
use crate::backoff::{self, RetryPolicy};
use crate::error::{Error, ErrorCode, ErrorKind, Result, Service, describe_json_error};
use crate::http;
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
    /// Fails when the key is empty after trimming or is not printable ASCII without
    /// whitespace, or when the HTTP client cannot be built.
    pub fn try_new(api_key: &str) -> Result<Self> {
        Ok(Self {
            http: http::client(DEFAULT_BASE_URL, builder())?,
            authorization: http::bearer(Service::Firecrawl, api_key)?,
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
        (self.base_url, self.http) =
            http::rebase(Service::Firecrawl, "base_url", base_url, builder())?;
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
    /// The request is validated before anything is sent. Whatever [`Error::is_retryable`]
    /// accepts (408, 409, 429, 5xx, connection failures) is retried, except timeouts and
    /// `Interrupted` (the response started, then stopped), because Firecrawl has already answered
    /// and may have billed. Each wait is the backoff delay or `Retry-After`, whichever is
    /// longer; a `Retry-After` over 30 seconds fails at once without retrying.
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
        let policy = RetryPolicy {
            max_retries: self.max_retries,
            max_retry_after: MAX_RETRY_AFTER,
            budget: None,
            resend_timeouts: false,
            resend_interrupted: false,
        };
        let retried = backoff::retry(
            &policy,
            |_, _| self.send_once(&url, body, timeout),
            |error, attempt, delay| {
                tracing::warn!(
                    target: "topcoat_si::firecrawl",
                    error = error as &dyn std::error::Error,
                    path,
                    attempt = attempt + 1,
                    sleep_seconds = delay.as_secs_f64(),
                    "Retrying Firecrawl request"
                );
            },
        )
        .await;
        retried.result
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
        let reply = http::send(request, Service::Firecrawl, |_| None).await?;
        if !reply.is_success() {
            return Err(http_error(
                reply.status,
                &reply.body,
                retry_after_from_headers(&reply.headers),
            ));
        }
        let bytes = reply.body;
        // The serde error quotes the bad value, which is response content, so only its position
        // is kept.
        let parsed: T = serde_json::from_slice(&bytes).map_err(|e| {
            Error::new(
                ErrorKind::Decode,
                format!(
                    "Failed to parse Firecrawl response ({} bytes): {}",
                    bytes.len(),
                    describe_json_error(&e)
                ),
            )
        })?;
        // A 2xx body saying it failed is a refusal without an HTTP error status, like a stream
        // `error` event, carrying Firecrawl's `error` text and `code`.
        if !parsed.succeeded() {
            let value = serde_json::from_slice::<serde_json::Value>(&bytes).ok();
            let field = |name: &str| {
                value
                    .as_ref()
                    .and_then(|value| value[name].as_str())
                    .map(String::from)
            };
            let kind = ErrorKind::Api {
                service: Service::Firecrawl,
                status: None,
                error_type: None,
                error_code: field("code").map(ErrorCode::Firecrawl),
                retry_after: None,
                retryable: None,
            };
            let message = field("error")
                .filter(|message| !message.trim().is_empty())
                .unwrap_or_else(|| "success false with no error message".to_string());
            return Err(Error::new(kind, message));
        }
        Ok(parsed)
    }
}

/// The Firecrawl settings for [`http::client`].
fn builder() -> ClientBuilder {
    ReqwestClient::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(DEFAULT_TIMEOUT)
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

fn http_error(status: u16, body: &[u8], retry_after: Option<Duration>) -> Error {
    let parsed = serde_json::from_slice::<serde_json::Value>(body).ok();
    let field = |name: &str| {
        parsed
            .as_ref()
            .and_then(|value| value[name].as_str())
            .map(String::from)
    };
    // Display already shows the service and status, so the message is Firecrawl's own words; its
    // `code` (such as `RATE_LIMIT`) is the error code that Display shows next to the status.
    Error::http_status(
        Service::Firecrawl,
        status,
        field("error"),
        None,
        field("code").map(ErrorCode::Firecrawl),
        None,
        retry_after,
    )
}
