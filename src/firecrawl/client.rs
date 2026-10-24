//! The Firecrawl HTTP client.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, HeaderValue};
use reqwest::{Client as ReqwestClient, ClientBuilder, Method};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::Semaphore;

use super::credit_usage::CreditUsageResponse;
use super::{
    CreditUsage, FirecrawlCache, GovSearchRequest, GovSearchResponse, MapRequest, MapResponse,
    ScrapeRequest, ScrapeResponse, SearchRequest, SearchResponse,
};
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

/// Client for Firecrawl's search, scrape, map and credit usage APIs.
///
/// The API key is only kept as a sensitive header value and is redacted from `Debug`.
#[derive(Clone)]
pub struct FirecrawlClient {
    pub(super) http: ReqwestClient,
    authorization: HeaderValue,
    pub(super) base_url: String,
    max_retries: usize,
    cache: Option<(Arc<dyn FirecrawlCache>, Duration)>,
    // Shared by clones, so they count against the same limit.
    concurrency: Option<(Arc<Semaphore>, usize)>,
}

impl fmt::Debug for FirecrawlClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FirecrawlClient")
            .field("api_key", &"[REDACTED]")
            .field("base_url", &self.base_url)
            .field("max_retries", &self.max_retries)
            .field("cache", &self.cache.is_some())
            .field(
                "max_concurrency",
                &self.concurrency.as_ref().map(|(_, max)| *max),
            )
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
            cache: None,
            concurrency: None,
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

    /// Keep answers in `cache` for `ttl`, so an identical search, gov search, scrape or map is
    /// paid for once. Credit usage is never cached.
    ///
    /// The key is the request body as sent, so any difference in the request is a miss. Failures
    /// are never cached, and a cache that errors only costs a repeat call.
    #[must_use]
    pub fn with_cache(mut self, cache: Arc<dyn FirecrawlCache>, ttl: Duration) -> Self {
        self.cache = Some((cache, ttl));
        self
    }

    /// Keep at most `max` Firecrawl requests open at once, across this client and its clones.
    /// Unlimited by default. A `max` of 0 is taken as 1.
    ///
    /// Every call that reaches Firecrawl (search, gov search, scrape, map and credit usage)
    /// waits for a free slot before each attempt and gives it back when the attempt ends, so a
    /// retry's backoff does not hold one. An answer served from
    /// [`FirecrawlClient::with_cache`] never waits. There is no requests-per-minute pacing: set
    /// `max` to your plan's concurrent browsers and let 429s be retried.
    ///
    /// The HTTP timeout starts once a slot is free, so time spent waiting for one counts
    /// against any timeout you put around the call, not against the request's own timeout.
    #[must_use]
    pub fn with_max_concurrency(mut self, max: usize) -> Self {
        let max = max.clamp(1, Semaphore::MAX_PERMITS);
        self.concurrency = Some((Arc::new(Semaphore::new(max)), max));
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
        let key = cache_key("search:", &request.wire());
        if let Some(mut hit) = self.cached::<SearchResponse>(key.as_deref()).await {
            // A cached answer costs nothing, so a credit budget must not be charged again. An
            // answer that reported no credits stays as it was.
            if hit.credits_used.is_some() {
                hit.credits_used = Some(0);
            }
            return Ok(hit);
        }
        let response = self.post("/v2/search", &request.wire(), None).await?;
        self.store(key.as_deref(), &response).await;
        Ok(response)
    }

    /// Scrape one page as markdown.
    ///
    /// The request is validated before anything is sent and retried like
    /// [`FirecrawlClient::search`]. A page that loaded with an error status (such as 404) is
    /// still a successful scrape: check
    /// [`ResultMetadata::status_code`](super::ResultMetadata::status_code).
    ///
    /// When the scrape sets a timeout, the HTTP timeout is that plus 30 seconds, and at least
    /// 120 seconds. [`ScrapeData::credits`](super::ScrapeData::credits) says what it cost.
    pub async fn scrape(&self, request: &ScrapeRequest) -> Result<ScrapeResponse> {
        request.validate()?;
        let timeout = scrape_http_timeout(request.options.timeout);
        let key = cache_key("scrape:", &request.wire());
        // `max_age` of zero asks for a fresh page, so the lookup is skipped.
        if request.options.max_age != Some(Duration::ZERO)
            && let Some(mut hit) = self.cached::<ScrapeResponse>(key.as_deref()).await
        {
            // A cached answer costs nothing, so it says so even when Firecrawl sent no figure.
            hit.data
                .metadata
                .get_or_insert_with(Default::default)
                .credits_used = Some(0);
            return Ok(hit);
        }
        let response: ScrapeResponse = self.post("/v2/scrape", &request.wire(), timeout).await?;
        // A page that answered with an error status is a failure, so it is not cached either.
        let page_failed = response
            .data
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.status_code)
            .is_some_and(|status| status >= 400);
        if !page_failed {
            self.store(key.as_deref(), &response).await;
        }
        Ok(response)
    }

    /// The cached response for `key`; a missing, stale or unreadable entry is a miss.
    async fn cached<T: DeserializeOwned>(&self, key: Option<&str>) -> Option<T> {
        let ((cache, _), key) = self.cache.as_ref().zip(key)?;
        serde_json::from_str(&cache.get(key).await?).ok()
    }

    /// Caches `response` under `key`; a response that will not serialize is not cached.
    async fn store<T: Serialize>(&self, key: Option<&str>, response: &T) {
        let Some(((cache, ttl), key)) = self.cache.as_ref().zip(key) else {
            return;
        };
        if let Ok(body) = serde_json::to_string(response) {
            cache.put(key, body, *ttl).await;
        }
    }

    /// Search Firecrawl's Government Index of statutes, regulations, codes and court opinions.
    ///
    /// The request is validated before anything is sent and retried like
    /// [`FirecrawlClient::search`], so an upstream 502 or 504 is resent. No results does not
    /// mean no law: filters only match pages Firecrawl has labeled.
    pub async fn gov_search(&self, request: &GovSearchRequest) -> Result<GovSearchResponse> {
        request.validate()?;
        let key = cache_key("gov_search:", &request.wire());
        if let Some(hit) = self.cached(key.as_deref()).await {
            return Ok(hit);
        }
        let response = self.post("/v2/search/gov", &request.wire(), None).await?;
        self.store(key.as_deref(), &response).await;
        Ok(response)
    }

    /// List the URLs of a site, from its sitemap and links. One credit per call, however many
    /// URLs come back; an answer served from [`FirecrawlClient::with_cache`] costs none and says
    /// so in [`MapResponse::credits_used`].
    ///
    /// The request is validated before anything is sent and retried like
    /// [`FirecrawlClient::search`]. Map is fast and may miss links.
    pub async fn map(&self, request: &MapRequest) -> Result<MapResponse> {
        request.validate()?;
        let key = cache_key("map:", &request.wire());
        if let Some(mut hit) = self.cached::<MapResponse>(key.as_deref()).await {
            // A cached answer costs nothing, so a credit budget must not be charged again.
            hit.credits_used = Some(0);
            return Ok(hit);
        }
        let response = self.post("/v2/map", &request.wire(), None).await?;
        self.store(key.as_deref(), &response).await;
        Ok(response)
    }

    /// The team's remaining credits and current billing period. Costs no credits.
    ///
    /// Retried like [`FirecrawlClient::search`], and also after a timeout or a response that
    /// stopped partway, because reading the balance bills nothing.
    pub async fn credit_usage(&self) -> Result<CreditUsage> {
        let response: CreditUsageResponse = self
            .send(Method::GET, "/v2/team/credit-usage", None::<&()>, None)
            .await?;
        response.data.ok_or_else(|| {
            Error::new(
                ErrorKind::Decode,
                "Firecrawl credit usage response has no data",
            )
        })
    }

    /// Posts `body` to `path`, retrying transient failures.
    async fn post<B, T>(&self, path: &str, body: &B, timeout: Option<Duration>) -> Result<T>
    where
        B: Serialize + ?Sized,
        T: DeserializeOwned + Succeeded,
    {
        self.send(Method::POST, path, Some(body), timeout).await
    }

    /// Sends `body`, if any, to `path`, retrying transient failures.
    async fn send<B, T>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
        timeout: Option<Duration>,
    ) -> Result<T>
    where
        B: Serialize + ?Sized,
        T: DeserializeOwned + Succeeded,
    {
        let url = format!("{}{path}", self.base_url);
        // A GET reads without billing, so it is safe to resend after any failure.
        let idempotent = method == Method::GET;
        let policy = RetryPolicy {
            max_retries: self.max_retries,
            max_retry_after: MAX_RETRY_AFTER,
            budget: None,
            resend_timeouts: idempotent,
            resend_interrupted: idempotent,
        };
        let retried = backoff::retry(
            &policy,
            |_, _| self.send_once(method.clone(), &url, body, timeout),
            |error, attempt, delay| {
                tracing::warn!(
                    target: "carpa::firecrawl",
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

    async fn send_once<B, T>(
        &self,
        method: Method,
        url: &str,
        body: Option<&B>,
        timeout: Option<Duration>,
    ) -> Result<T>
    where
        B: Serialize + ?Sized,
        T: DeserializeOwned + Succeeded,
    {
        // Held until the response is read; a closed semaphore (never closed here) limits nothing.
        let _slot = match &self.concurrency {
            Some((semaphore, _)) => semaphore.acquire().await.ok(),
            None => None,
        };
        let mut request = self
            .http
            .request(method, url)
            .header(AUTHORIZATION, self.authorization.clone());
        if let Some(body) = body {
            request = request.json(body);
        }
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

/// The HTTP timeout for a scrape with server-side `timeout`: Firecrawl may work for all of it,
/// so the response gets a margin to arrive.
pub(super) fn scrape_http_timeout(timeout: Option<Duration>) -> Option<Duration> {
    timeout.map(|t| (t + SCRAPE_TIMEOUT_MARGIN).max(DEFAULT_TIMEOUT))
}

/// The cache key for a request: `prefix` plus its wire body, or `None` when it will not serialize.
fn cache_key(prefix: &str, wire: &impl Serialize) -> Option<String> {
    Some(format!("{prefix}{}", serde_json::to_string(wire).ok()?))
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

impl Succeeded for GovSearchResponse {
    fn succeeded(&self) -> bool {
        self.success
    }
}

impl Succeeded for MapResponse {
    fn succeeded(&self) -> bool {
        self.success
    }
}

impl Succeeded for CreditUsageResponse {
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
