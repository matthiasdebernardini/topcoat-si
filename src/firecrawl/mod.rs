//! Web search and page fetching through Firecrawl's v2 API.
//!
//! [`FirecrawlClient`] is a typed client for `POST /v2/search` and `POST /v2/scrape`.
//! [`FirecrawlWebSearch`] wraps search as a client-side `web_search` tool: hand
//! [`FirecrawlWebSearch::tool`] to the model and answer its calls with
//! [`FirecrawlWebSearch::execute`]. [`FirecrawlFetchPage`] wraps scrape as a client-side
//! `fetch_page` tool through [`ClientTool`](crate::ClientTool).

use std::time::Duration;

use reqwest::Client as ReqwestClient;
use reqwest::header::{AUTHORIZATION, HeaderValue};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use time::Date;
use tokio::time::sleep;

use crate::backoff::ExponentialBackoff;
use crate::error::{Error, HttpMetadata, RequestPhase, Result, map_reqwest_error};
use crate::types::retry_after_from_headers;

mod fetch_page;
mod web_search;

#[cfg(test)]
mod tests;

pub use fetch_page::FirecrawlFetchPage;
pub use web_search::{FirecrawlWebSearch, WEB_SEARCH_TOOL_NAME};

const DEFAULT_BASE_URL: &str = "https://api.firecrawl.dev";
const API_KEY_ENV: &str = "FIRECRAWL_API_KEY";
// Scraping each result can take tens of seconds, so allow well beyond Firecrawl's own default.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
// Waiting out a longer Retry-After would stall the conversation, so fail right away instead.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(30);
const MAX_QUERY_CHARS: usize = 500;
// Slack between Firecrawl's server-side scrape timeout and the client-side request timeout.
const SCRAPE_TIMEOUT_MARGIN: Duration = Duration::from_secs(30);
const MIN_SCRAPE_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_SCRAPE_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_PDF_PAGES: u32 = 10_000;

/// Client for Firecrawl's search and scrape APIs.
///
/// The API key is only kept as a sensitive header value and is redacted from `Debug`.
#[derive(Clone)]
pub struct FirecrawlClient {
    http: ReqwestClient,
    authorization: HeaderValue,
    base_url: String,
    max_retries: usize,
}

impl std::fmt::Debug for FirecrawlClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FirecrawlClient")
            .field("api_key", &"[REDACTED]")
            .field("base_url", &self.base_url)
            .field("max_retries", &self.max_retries)
            .finish_non_exhaustive()
    }
}

impl FirecrawlClient {
    /// Create a client with the given Firecrawl API key.
    pub fn new(api_key: impl Into<String>) -> Result<Self> {
        let api_key = api_key.into();
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
        let http = ReqwestClient::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(30))
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .map_err(|e| {
                Error::http_client(
                    format!("Failed to build HTTP client: {e}"),
                    Some(Box::new(e)),
                )
            })?;
        Ok(Self {
            http,
            authorization,
            base_url: DEFAULT_BASE_URL.to_string(),
            max_retries: 2,
        })
    }

    /// Create a client from the `FIRECRAWL_API_KEY` environment variable.
    pub fn from_env() -> Result<Self> {
        match std::env::var(API_KEY_ENV) {
            Ok(key) if !key.trim().is_empty() => Self::new(key),
            _ => Err(Error::validation(
                format!("{API_KEY_ENV} is not set or empty"),
                Some(API_KEY_ENV.to_string()),
            )),
        }
    }

    /// Send requests to another base URL, such as a local test server.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into().trim_end_matches('/').to_string();
        self
    }

    /// Set how many times a retryable failure is retried. Defaults to 2.
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
    /// still a successful scrape: check [`ResultMetadata::status_code`].
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
                .unwrap_or_else(|| ExponentialBackoff::next(attempt));
            tracing::warn!(
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
        error_type: code,
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
            duration: None,
            source: None,
        },
        _ => Error::Api {
            message,
            metadata,
            source: None,
        },
    }
}

/// Where Firecrawl searches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchSource {
    /// Regular web results.
    Web,
    /// News articles.
    News,
    /// Images.
    Images,
}

/// Restricts results to a kind of site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchCategory {
    /// Code hosts and developer documentation. Cannot be combined with other categories.
    Developer,
    /// Research papers and academic sites.
    Research,
    /// PDF documents.
    Pdf,
    /// Government sites. Cannot be combined with other categories.
    Gov,
}

/// Limits results to a publication window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeRange {
    /// The past hour.
    PastHour,
    /// The past day.
    PastDay,
    /// The past week.
    PastWeek,
    /// The past month.
    PastMonth,
    /// The past year.
    PastYear,
    /// An inclusive date range.
    Custom {
        /// First day of the range.
        from: Date,
        /// Last day of the range.
        to: Date,
    },
}

impl TimeRange {
    fn tbs(&self) -> String {
        let us_date = |date: &Date| {
            format!(
                "{:02}/{:02}/{:04}",
                u8::from(date.month()),
                date.day(),
                date.year()
            )
        };
        match self {
            Self::PastHour => "qdr:h".to_string(),
            Self::PastDay => "qdr:d".to_string(),
            Self::PastWeek => "qdr:w".to_string(),
            Self::PastMonth => "qdr:m".to_string(),
            Self::PastYear => "qdr:y".to_string(),
            Self::Custom { from, to } => {
                format!("cdr:1,cd_min:{},cd_max:{}", us_date(from), us_date(to))
            }
        }
    }
}

/// How a page is scraped as markdown, for a search result or a single scrape.
///
/// Every scraped search result costs one extra Firecrawl credit. A PDF costs one credit per
/// page read, so cap long documents with [`ScrapeOptions::with_pdf_max_pages`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrapeOptions {
    /// Strip navigation, headers and footers from the page.
    pub only_main_content: Option<bool>,
    /// Accept a cached copy of the page up to this old.
    pub max_age: Option<Duration>,
    /// Server-side time limit for the scrape, 1 to 300 seconds. Firecrawl defaults to 60.
    pub timeout: Option<Duration>,
    /// Read at most this many PDF pages, 1 to 10,000.
    ///
    /// Firecrawl bills 1 credit per PDF page. Setting this sends a parsers list holding only
    /// the PDF parser, which switches off the parsers it omits (image OCR).
    pub pdf_max_pages: Option<u32>,
}

impl ScrapeOptions {
    /// Scrape markdown with Firecrawl's defaults for everything else.
    pub fn new() -> Self {
        Self {
            only_main_content: None,
            max_age: None,
            timeout: None,
            pdf_max_pages: None,
        }
    }

    /// Keep only the main content of each page.
    pub fn with_only_main_content(mut self, only_main_content: bool) -> Self {
        self.only_main_content = Some(only_main_content);
        self
    }

    /// Accept cached pages up to this age.
    pub fn with_max_age(mut self, max_age: Duration) -> Self {
        self.max_age = Some(max_age);
        self
    }

    /// Set the server-side time limit for the scrape.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Read at most this many PDF pages. Also switches off image OCR, see
    /// [`ScrapeOptions::pdf_max_pages`].
    pub fn with_pdf_max_pages(mut self, pages: u32) -> Self {
        self.pdf_max_pages = Some(pages);
        self
    }

    fn validate(&self) -> Result<()> {
        let invalid =
            |message: &str, param: &str| Err(Error::validation(message, Some(param.to_string())));
        if self
            .timeout
            .is_some_and(|t| !(MIN_SCRAPE_TIMEOUT..=MAX_SCRAPE_TIMEOUT).contains(&t))
        {
            return invalid(
                "scrape timeout must be between 1 and 300 seconds",
                "timeout",
            );
        }
        if self
            .pdf_max_pages
            .is_some_and(|pages| !(1..=MAX_PDF_PAGES).contains(&pages))
        {
            return invalid("pdf_max_pages must be between 1 and 10000", "pdf_max_pages");
        }
        Ok(())
    }

    fn wire(&self) -> WireScrapeOptions {
        WireScrapeOptions {
            formats: vec!["markdown"],
            only_main_content: self.only_main_content,
            max_age: self.max_age.map(millis),
            timeout: self.timeout.map(millis),
            parsers: self.pdf_max_pages.map(|max_pages| {
                vec![WireParser {
                    kind: "pdf",
                    max_pages,
                }]
            }),
        }
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

impl Default for ScrapeOptions {
    fn default() -> Self {
        Self::new()
    }
}

/// Parameters for [`FirecrawlClient::search`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRequest {
    /// The search query, at most 500 characters.
    pub query: String,
    /// Results per source, 1 to 100. Firecrawl defaults to 10.
    pub limit: Option<u32>,
    /// Sources to search. Empty means Firecrawl's default (web).
    pub sources: Vec<SearchSource>,
    /// Categories to restrict results to.
    pub categories: Vec<SearchCategory>,
    /// Only return results from these hostnames. Exclusive with `exclude_domains`.
    pub include_domains: Vec<String>,
    /// Never return results from these hostnames. Exclusive with `include_domains`.
    pub exclude_domains: Vec<String>,
    /// Publication window.
    pub time_range: Option<TimeRange>,
    /// Order results by date instead of relevance.
    pub sort_by_date: bool,
    /// Location to search from, such as `"Germany"` or `"San Francisco,California,United States"`.
    pub location: Option<String>,
    /// ISO country code to search from, such as `"US"`.
    pub country: Option<String>,
    /// Server-side time limit for the whole search.
    pub timeout: Option<Duration>,
    /// Drop invalid URLs from results instead of failing.
    pub ignore_invalid_urls: Option<bool>,
    /// Scrape each result's page.
    pub scrape_options: Option<ScrapeOptions>,
}

impl SearchRequest {
    /// Create a request for `query` with Firecrawl's defaults.
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            limit: None,
            sources: Vec::new(),
            categories: Vec::new(),
            include_domains: Vec::new(),
            exclude_domains: Vec::new(),
            time_range: None,
            sort_by_date: false,
            location: None,
            country: None,
            timeout: None,
            ignore_invalid_urls: None,
            scrape_options: None,
        }
    }

    /// Set the number of results per source.
    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Set the sources to search.
    pub fn with_sources(mut self, sources: Vec<SearchSource>) -> Self {
        self.sources = sources;
        self
    }

    /// Set the categories to restrict results to.
    pub fn with_categories(mut self, categories: Vec<SearchCategory>) -> Self {
        self.categories = categories;
        self
    }

    /// Only return results from these hostnames.
    pub fn with_include_domains(mut self, domains: Vec<String>) -> Self {
        self.include_domains = domains;
        self
    }

    /// Never return results from these hostnames.
    pub fn with_exclude_domains(mut self, domains: Vec<String>) -> Self {
        self.exclude_domains = domains;
        self
    }

    /// Set the publication window.
    pub fn with_time_range(mut self, time_range: TimeRange) -> Self {
        self.time_range = Some(time_range);
        self
    }

    /// Order results by date instead of relevance.
    pub fn with_sort_by_date(mut self, sort_by_date: bool) -> Self {
        self.sort_by_date = sort_by_date;
        self
    }

    /// Set the location to search from.
    pub fn with_location(mut self, location: impl Into<String>) -> Self {
        self.location = Some(location.into());
        self
    }

    /// Set the ISO country code to search from.
    pub fn with_country(mut self, country: impl Into<String>) -> Self {
        self.country = Some(country.into());
        self
    }

    /// Set the server-side time limit.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Drop invalid URLs from results instead of failing.
    pub fn with_ignore_invalid_urls(mut self, ignore: bool) -> Self {
        self.ignore_invalid_urls = Some(ignore);
        self
    }

    /// Scrape each result's page.
    pub fn with_scrape_options(mut self, scrape_options: ScrapeOptions) -> Self {
        self.scrape_options = Some(scrape_options);
        self
    }

    /// Check the constraints Firecrawl would otherwise reject with a 400.
    pub fn validate(&self) -> Result<()> {
        let invalid =
            |message: &str, param: &str| Err(Error::validation(message, Some(param.to_string())));
        if self.query.trim().is_empty() {
            return invalid("query must not be empty", "query");
        }
        if self.query.chars().count() > MAX_QUERY_CHARS {
            return invalid("query must be at most 500 characters", "query");
        }
        if self.limit.is_some_and(|limit| !(1..=100).contains(&limit)) {
            return invalid("limit must be between 1 and 100", "limit");
        }
        if !self.include_domains.is_empty() && !self.exclude_domains.is_empty() {
            return invalid(
                "include_domains and exclude_domains are mutually exclusive",
                "include_domains",
            );
        }
        let exclusive = self
            .categories
            .iter()
            .any(|c| matches!(c, SearchCategory::Gov | SearchCategory::Developer));
        if exclusive && self.categories.len() > 1 {
            return invalid(
                "the gov and developer categories cannot be combined with other categories",
                "categories",
            );
        }
        if let Some(TimeRange::Custom { from, to }) = self.time_range
            && from > to
        {
            return invalid("custom time range starts after it ends", "time_range");
        }
        if let Some(scrape_options) = &self.scrape_options {
            scrape_options.validate()?;
        }
        Ok(())
    }

    fn wire(&self) -> WireSearchRequest<'_> {
        let tbs: Vec<String> = self
            .time_range
            .iter()
            .map(TimeRange::tbs)
            .chain(self.sort_by_date.then(|| "sbd:1".to_string()))
            .collect();
        WireSearchRequest {
            query: &self.query,
            limit: self.limit,
            sources: &self.sources,
            categories: &self.categories,
            include_domains: &self.include_domains,
            exclude_domains: &self.exclude_domains,
            tbs: (!tbs.is_empty()).then(|| tbs.join(",")),
            location: self.location.as_deref(),
            country: self.country.as_deref(),
            timeout: self.timeout.map(millis),
            ignore_invalid_urls: self.ignore_invalid_urls,
            scrape_options: self.scrape_options.as_ref().map(ScrapeOptions::wire),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireSearchRequest<'a> {
    query: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit: Option<u32>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    sources: &'a [SearchSource],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    categories: &'a [SearchCategory],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    include_domains: &'a [String],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    exclude_domains: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    tbs: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    country: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout: Option<u64>,
    #[serde(rename = "ignoreInvalidURLs", skip_serializing_if = "Option::is_none")]
    ignore_invalid_urls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scrape_options: Option<WireScrapeOptions>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireScrapeOptions {
    formats: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    only_main_content: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_age: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parsers: Option<Vec<WireParser>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireParser {
    #[serde(rename = "type")]
    kind: &'static str,
    max_pages: u32,
}

/// Parameters for [`FirecrawlClient::scrape`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrapeRequest {
    /// Absolute http or https URL of the page.
    pub url: String,
    /// How the page is scraped. Markdown is always requested.
    pub options: ScrapeOptions,
}

impl ScrapeRequest {
    /// Create a request for `url` with Firecrawl's defaults.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            options: ScrapeOptions::new(),
        }
    }

    /// Set how the page is scraped.
    pub fn with_options(mut self, options: ScrapeOptions) -> Self {
        self.options = options;
        self
    }

    /// Check the constraints Firecrawl would otherwise reject with a 400.
    pub fn validate(&self) -> Result<()> {
        let absolute = url::Url::parse(self.url.trim())
            .ok()
            .filter(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some());
        if absolute.is_none() {
            return Err(Error::validation(
                "url must be an absolute http or https URL",
                Some("url".to_string()),
            ));
        }
        self.options.validate()
    }

    fn wire(&self) -> WireScrapeRequest<'_> {
        WireScrapeRequest {
            url: self.url.trim(),
            options: self.options.wire(),
        }
    }
}

#[derive(Serialize)]
struct WireScrapeRequest<'a> {
    url: &'a str,
    #[serde(flatten)]
    options: WireScrapeOptions,
}

/// A successful Firecrawl scrape.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(from = "WireScrapeResponse")]
pub struct ScrapeResponse {
    /// Whether the scrape succeeded.
    pub success: bool,
    /// The scraped page. Empty when Firecrawl sent no data.
    pub data: ScrapeData,
}

/// A scraped page.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ScrapeData {
    /// Page content as markdown.
    #[serde(default)]
    pub markdown: Option<String>,
    /// Page metadata, including the final URL and the page's own HTTP status.
    #[serde(default)]
    pub metadata: Option<ResultMetadata>,
    /// Non-fatal problem Firecrawl reported. Read from `data.warning`, else the top-level
    /// `warning`.
    #[serde(default)]
    pub warning: Option<String>,
}

#[derive(Deserialize)]
struct WireScrapeResponse {
    success: bool,
    #[serde(default)]
    data: Option<ScrapeData>,
    #[serde(default)]
    warning: Option<String>,
}

impl From<WireScrapeResponse> for ScrapeResponse {
    fn from(wire: WireScrapeResponse) -> Self {
        let mut data = wire.data.unwrap_or_default();
        if data.warning.is_none() {
            data.warning = wire.warning;
        }
        Self {
            success: wire.success,
            data,
        }
    }
}

/// A successful Firecrawl search.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    /// Whether the search succeeded.
    pub success: bool,
    /// Search job id.
    #[serde(default)]
    pub id: Option<String>,
    /// Credits the search consumed.
    #[serde(default)]
    pub credits_used: Option<u64>,
    /// Non-fatal problem Firecrawl reported, such as a partially failed scrape.
    #[serde(default)]
    pub warning: Option<String>,
    /// Results grouped by source.
    #[serde(default)]
    pub data: Option<SearchData>,
}

/// Results grouped by source.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct SearchData {
    /// Web results.
    #[serde(default)]
    pub web: Option<Vec<WebResult>>,
    /// News results.
    #[serde(default)]
    pub news: Option<Vec<NewsResult>>,
    /// Image results.
    #[serde(default)]
    pub images: Option<Vec<ImageResult>>,
}

/// A web search result.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct WebResult {
    /// Page URL.
    #[serde(default)]
    pub url: Option<String>,
    /// Page title.
    #[serde(default)]
    pub title: Option<String>,
    /// Search engine snippet.
    #[serde(default)]
    pub description: Option<String>,
    /// Rank within the results.
    #[serde(default)]
    pub position: Option<u32>,
    /// Category the result matched.
    #[serde(default)]
    pub category: Option<String>,
    /// Scraped page content, when scraping was requested.
    #[serde(default)]
    pub markdown: Option<String>,
    /// Scrape metadata.
    #[serde(default)]
    pub metadata: Option<ResultMetadata>,
}

/// A news search result.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct NewsResult {
    /// Article URL.
    #[serde(default)]
    pub url: Option<String>,
    /// Article title.
    #[serde(default)]
    pub title: Option<String>,
    /// Article snippet.
    #[serde(default)]
    pub snippet: Option<String>,
    /// Publication date as reported by the search engine.
    #[serde(default)]
    pub date: Option<String>,
    /// Rank within the results.
    #[serde(default)]
    pub position: Option<u32>,
    /// Scraped article content, when scraping was requested.
    #[serde(default)]
    pub markdown: Option<String>,
    /// Scrape metadata.
    #[serde(default)]
    pub metadata: Option<ResultMetadata>,
}

/// An image search result.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageResult {
    /// Page the image appears on.
    #[serde(default)]
    pub url: Option<String>,
    /// Image title.
    #[serde(default)]
    pub title: Option<String>,
    /// Direct image URL.
    #[serde(default)]
    pub image_url: Option<String>,
    /// Image width in pixels.
    #[serde(default)]
    pub image_width: Option<u32>,
    /// Image height in pixels.
    #[serde(default)]
    pub image_height: Option<u32>,
    /// Rank within the results.
    #[serde(default)]
    pub position: Option<u32>,
}

/// Metadata from scraping a page.
///
/// Parsing never fails: a field that is missing or of an unexpected type is `None`. Title and
/// description may arrive as an array of strings, in which case the first is kept.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResultMetadata {
    /// Page title.
    pub title: Option<String>,
    /// Page description.
    pub description: Option<String>,
    /// URL that was requested.
    pub source_url: Option<String>,
    /// URL after redirects.
    pub url: Option<String>,
    /// HTTP status of the scraped page.
    pub status_code: Option<u16>,
    /// Scrape error for this page.
    pub error: Option<String>,
    /// MIME type of the page, such as `text/html` or `application/pdf`.
    pub content_type: Option<String>,
    /// For a PDF, the number of pages read (capped by the PDF page limit).
    pub num_pages: Option<u32>,
    /// For a PDF, the document's full page count, when Firecrawl knows it.
    pub total_pages: Option<u32>,
}

impl<'de> Deserialize<'de> for ResultMetadata {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let text = |name: &str| value.get(name).and_then(first_string);
        let number = |name: &str| value.get(name).and_then(serde_json::Value::as_u64);
        Ok(Self {
            title: text("title"),
            description: text("description"),
            source_url: text("sourceURL"),
            url: text("url"),
            status_code: number("statusCode").and_then(|n| u16::try_from(n).ok()),
            error: text("error"),
            content_type: text("contentType"),
            num_pages: number("numPages").and_then(|n| u32::try_from(n).ok()),
            total_pages: number("totalPages").and_then(|n| u32::try_from(n).ok()),
        })
    }
}

/// A string, or the first element of an array of strings.
fn first_string(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Array(items) => items.first().and_then(first_string),
        _ => None,
    }
}
