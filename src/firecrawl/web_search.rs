//! The client-side `web_search` tool backed by Firecrawl.

use std::collections::HashSet;

use futures::future::BoxFuture;
use serde::Deserialize;
use serde_json::json;

use super::filter::{self, Filter};
use super::{
    FirecrawlClient, ScrapeOptions, SearchRequest, SearchResponse, SearchSource, TimeRange,
    tool_result,
};
#[cfg(feature = "code-mode")]
use super::{GovResult, GovSearchRequest, SearchCategory, WebResult};
use crate::client_tool::ClientTool;
use crate::error::{Error, ErrorKind, Result};
use crate::types::{Content, TextBlock, ToolParam, ToolResultBlock, ToolUnionParam, ToolUseBlock};

/// Name the model calls the Firecrawl search tool by.
pub const WEB_SEARCH_TOOL_NAME: &str = "web_search";

// Caps results per call across all sources, bounding both context size and Firecrawl credits.
const MAX_MODEL_LIMIT: u32 = 10;

/// Firecrawl web search exposed to the model as a client-side `web_search` tool.
///
/// Domain lists set here are enforced on every call: the model can narrow the allowed domains
/// and add blocked ones, never widen them, and a blocked domain always wins. Entries are
/// hostnames; a leading `*.` or `.` is accepted and means the same as the bare domain, which
/// always covers its subdomains.
#[derive(Debug, Clone)]
pub struct FirecrawlWebSearch {
    client: FirecrawlClient,
    default_limit: u32,
    scrape_options: Option<ScrapeOptions>,
    max_content_chars: usize,
    country: Option<String>,
    location: Option<String>,
    allowed_domains: Vec<String>,
    blocked_domains: Vec<String>,
}

impl FirecrawlWebSearch {
    /// Wrap a client with the defaults: 5 results, main-content markdown cut at 8000 characters.
    #[must_use]
    pub fn new(client: FirecrawlClient) -> Self {
        Self {
            client,
            default_limit: 5,
            scrape_options: Some(ScrapeOptions::new().with_only_main_content(true)),
            max_content_chars: 8000,
            country: None,
            location: None,
            allowed_domains: Vec::new(),
            blocked_domains: Vec::new(),
        }
    }

    /// Results returned when the model does not ask for a number, 1 to 10.
    #[must_use]
    pub fn with_default_limit(mut self, limit: u32) -> Self {
        self.default_limit = limit;
        self
    }

    /// Scraping applied to each result, or `None` to return search snippets only.
    #[must_use]
    pub fn with_scrape_options(mut self, scrape_options: Option<ScrapeOptions>) -> Self {
        self.scrape_options = scrape_options;
        self
    }

    /// Characters of page content kept per result.
    #[must_use]
    pub fn with_max_content_chars(mut self, max_content_chars: usize) -> Self {
        self.max_content_chars = max_content_chars;
        self
    }

    /// Country searched from when the model does not pick one.
    #[must_use]
    pub fn with_country(mut self, country: impl Into<String>) -> Self {
        self.country = Some(country.into());
        self
    }

    /// Location searched from.
    #[must_use]
    pub fn with_location(mut self, location: impl Into<String>) -> Self {
        self.location = Some(location.into());
        self
    }

    /// Only ever return results from these hostnames.
    #[must_use]
    pub fn with_allowed_domains(mut self, domains: Vec<String>) -> Self {
        self.allowed_domains = domains;
        self
    }

    /// Never return results from these hostnames.
    #[must_use]
    pub fn with_blocked_domains(mut self, domains: Vec<String>) -> Self {
        self.blocked_domains = domains;
        self
    }

    /// The tool definition to send to the model.
    pub fn tool(&self) -> ToolUnionParam {
        let hostnames = json!({
            "type": "array",
            "items": {"type": "string"},
        });
        let schema = json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query, at most 500 characters."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_MODEL_LIMIT,
                    "description": "Total number of results across all sources."
                },
                "recency": {
                    "type": "string",
                    "enum": ["hour", "day", "week", "month", "year"],
                    "description": "Only return results published within this past period."
                },
                "sources": {
                    "type": "array",
                    "items": {"type": "string", "enum": ["web", "news"]},
                    "description": "Where to search. Defaults to web."
                },
                "include_domains": merge_description(&hostnames, "Only return results from these hostnames, such as \"docs.rs\"."),
                "exclude_domains": merge_description(&hostnames, "Never return results from these hostnames."),
                "country": {
                    "type": "string",
                    "description": "ISO 3166-1 alpha-2 country code to search from, such as \"US\"."
                }
            },
            "required": ["query"],
            "additionalProperties": false
        });
        ToolUnionParam::CustomTool(
            ToolParam::new(WEB_SEARCH_TOOL_NAME.to_string(), schema)
                .with_description(self.description()),
        )
    }

    fn description(&self) -> String {
        let contents = if let Some(scrape) = &self.scrape_options {
            let content = if scrape.only_main_content == Some(true) {
                "main content"
            } else {
                "page content"
            };
            format!(
                "Each result carries the page URL, title, a short description and, when \
                 available, the page's {content} as markdown (cut off after {} characters), \
                 so you can usually answer and cite from the results without fetching the page.",
                self.max_content_chars
            )
        } else {
            "Each result carries the page URL, title and a short search snippet, not the page \
             content."
                .to_string()
        };
        format!(
            "Search the web. Use it for anything that may have changed since your training \
             data, for facts you are not sure of, and whenever the user asks about current \
             events, prices, releases, documentation or wants sources. {contents} Write the \
             query as you would type it into a search engine and prefer several focused \
             searches over one broad one. Set recency (hour, day, week, month or year) when the \
             question is about recent events or asks for the latest information; leave it unset \
             for timeless topics. Use sources [\"news\"] for news coverage. Use include_domains \
             to search only specific sites and exclude_domains to drop sites. At most \
             {MAX_MODEL_LIMIT} results per call."
        )
    }

    /// Whether this tool answers `tool_use`.
    pub fn handles(&self, tool_use: &ToolUseBlock) -> bool {
        tool_use.name == WEB_SEARCH_TOOL_NAME
    }

    /// Run the search the model asked for.
    ///
    /// Problems the model or a retry can fix come back as an `is_error` tool result so the
    /// conversation continues. Invalid configuration on this tool, an invalid key (401) and
    /// exhausted credits (402) are returned as `Err` because only the developer can fix them.
    pub async fn execute(&self, tool_use: &ToolUseBlock) -> Result<ToolResultBlock> {
        let filter = self.checked_config()?;
        let id = tool_use.id.clone();
        let input = match ModelInput::deserialize(&tool_use.input) {
            Ok(input) => input,
            Err(error) => {
                return Ok(ToolResultBlock::error(
                    id,
                    format!(
                        "Invalid web_search input: {error}. Send an object with a \"query\" \
                         string and only the optional fields limit, recency, sources, \
                         include_domains, exclude_domains and country."
                    ),
                ));
            }
        };
        Ok(
            match self
                .search(&input, filter, self.scrape_options.as_ref())
                .await?
            {
                Ok((response, filter, total)) => {
                    self.result(id, &input.query, response, &filter, total)
                }
                Err(message) => ToolResultBlock::error(id, message),
            },
        )
    }

    /// Checks a sandbox `search(...)` call and plans its request, without sending anything:
    /// the same checks as [`FirecrawlWebSearch::execute`]. A `limit` over 10 is capped at 10.
    /// The inner `Err` is a message the model can act on; `Err` is fatal.
    #[cfg(feature = "code-mode")]
    pub(crate) fn prepare_search(
        &self,
        query: SearchQuery,
    ) -> Result<std::result::Result<PreparedSearch, String>> {
        let filter = self.checked_config()?;
        if query.scrape && self.scrape_options.is_none() {
            return Ok(Err(
                "scrape=True is not available: this search does not scrape pages.".to_string(),
            ));
        }
        let input = ModelInput {
            query: query.query,
            limit: query.limit.map(|limit| limit.min(MAX_MODEL_LIMIT)),
            recency: None,
            sources: None,
            include_domains: query.include_domains,
            exclude_domains: query.exclude_domains,
            country: query.country,
        };
        let scrape_options = self.scrape_options.as_ref().filter(|_| query.scrape);
        let (mut request, filter, total) = match self.plan(&input, filter, scrape_options) {
            Ok(planned) => planned,
            Err(message) => return Ok(Err(message)),
        };
        request.time_range = query.time_range;
        request.sort_by_date = query.sort_by_date;
        request.categories = query.categories;
        if let Err(error) = request.validate() {
            return Ok(Err(error.to_string()));
        }
        Ok(Ok(PreparedSearch {
            request,
            filter,
            total,
        }))
    }

    /// Sends a search [`FirecrawlWebSearch::prepare_search`] planned. Returns the web results
    /// the domain lists permit, markdown cut to the tool's limit and only when scraped, and the
    /// credits the search cost. The inner `Err` is a message the model can act on; `Err` is
    /// fatal (401, 402).
    #[cfg(feature = "code-mode")]
    pub(crate) async fn send_search(
        &self,
        prepared: PreparedSearch,
    ) -> Result<std::result::Result<Searched, String>> {
        let response = match self.send(&prepared.request).await? {
            Ok(response) => response,
            Err(message) => return Ok(Err(message)),
        };
        let web = response.data.and_then(|data| data.web).unwrap_or_default();
        let scraped = web
            .iter()
            .filter(|result| result.markdown.is_some())
            .count();
        // Firecrawl bills 2 credits per 10 results, at least 2 even for none, and 1 for each
        // scraped one.
        let estimate = 2 * u64::try_from(web.len().div_ceil(10).max(1)).unwrap_or(u64::MAX)
            + u64::try_from(scraped).unwrap_or(u64::MAX);
        let results = web
            .into_iter()
            .filter(|web| {
                web.url
                    .as_deref()
                    .is_some_and(|u| prepared.filter.permits(u))
            })
            .map(|mut web| {
                web.markdown = web
                    .markdown
                    .map(|m| tool_result::truncate(&m, self.max_content_chars));
                web
            })
            .take(usize::try_from(prepared.total).unwrap_or(usize::MAX))
            .collect();
        Ok(Ok(Searched {
            results,
            credits: response.credits_used.unwrap_or(estimate),
        }))
    }

    /// Checks a sandbox `gov_search(...)` call without sending anything. The inner `Err` is a
    /// message the model can act on; `Err` is fatal.
    #[cfg(feature = "code-mode")]
    pub(crate) fn prepare_gov_search(
        &self,
        request: GovSearchRequest,
    ) -> Result<std::result::Result<GovSearchRequest, String>> {
        self.checked_config()?;
        Ok(request
            .validate()
            .map(|()| request)
            .map_err(|e| e.to_string()))
    }

    /// Sends a Government Index search, keeping the results the domain lists permit. The inner
    /// `Err` is a message the model can act on; `Err` is fatal (401, 402).
    #[cfg(feature = "code-mode")]
    pub(crate) async fn send_gov_search(
        &self,
        request: &GovSearchRequest,
    ) -> Result<std::result::Result<Vec<GovResult>, String>> {
        let filter = self.checked_config()?;
        match self.client.gov_search(request).await {
            Ok(response) => Ok(Ok(response
                .data
                .web
                .into_iter()
                .filter(|result| result.url.as_deref().is_some_and(|u| filter.permits(u)))
                .collect())),
            Err(error) if matches!(error.status(), Some(401 | 402)) => Err(error),
            Err(error) => Ok(Err(format!("Gov search failed: {error}"))),
        }
    }

    /// Runs one search. The inner `Err` is a message the model can act on; `Err` is fatal
    /// (401, 402).
    async fn search(
        &self,
        input: &ModelInput,
        filter: Filter,
        scrape_options: Option<&ScrapeOptions>,
    ) -> Result<std::result::Result<(SearchResponse, Filter, u32), String>> {
        let (request, filter, total) = match self.plan(input, filter, scrape_options) {
            Ok(planned) => planned,
            Err(message) => return Ok(Err(message)),
        };
        Ok(self
            .send(&request)
            .await?
            .map(|response| (response, filter, total)))
    }

    /// Checks the model's input and builds the request, the filter to apply to the results and
    /// how many results to keep.
    fn plan(
        &self,
        input: &ModelInput,
        filter: Filter,
        scrape_options: Option<&ScrapeOptions>,
    ) -> std::result::Result<(SearchRequest, Filter, u32), String> {
        if input
            .limit
            .is_some_and(|limit| !(1..=MAX_MODEL_LIMIT).contains(&limit))
        {
            return Err(format!("limit must be between 1 and {MAX_MODEL_LIMIT}."));
        }
        let (mut request, filter, total) = self.request(input, filter)?;
        request.scrape_options = scrape_options.cloned();
        Ok((request, filter, total))
    }

    /// Sends one search. The inner `Err` is a message the model can act on; `Err` is fatal
    /// (401, 402).
    async fn send(
        &self,
        request: &SearchRequest,
    ) -> Result<std::result::Result<SearchResponse, String>> {
        match self.client.search(request).await {
            Ok(response) => Ok(Ok(response)),
            Err(error) if matches!(error.status(), Some(401 | 402)) => Err(error),
            Err(error) if matches!(error.kind(), ErrorKind::Validation { .. }) => {
                Ok(Err(error.to_string()))
            }
            Err(error) => Ok(Err(format!("Web search failed: {error}"))),
        }
    }

    fn checked_config(&self) -> Result<Filter> {
        let invalid =
            |message: String, param: &str| Error::validation(message, Some(param.to_string()));
        if !(1..=MAX_MODEL_LIMIT).contains(&self.default_limit) {
            return Err(invalid(
                format!("default_limit must be between 1 and {MAX_MODEL_LIMIT}"),
                "default_limit",
            ));
        }
        if self.scrape_options.is_some() && self.max_content_chars == 0 {
            return Err(invalid(
                "max_content_chars must be positive; disable scraping instead".to_string(),
                "max_content_chars",
            ));
        }
        for (value, param) in [(&self.country, "country"), (&self.location, "location")] {
            if value.as_deref().is_some_and(|v| v.trim().is_empty()) {
                return Err(invalid(format!("{param} must not be empty"), param));
            }
        }
        Filter::try_new(&self.allowed_domains, &self.blocked_domains)
    }

    fn request(
        &self,
        input: &ModelInput,
        mut filter: Filter,
    ) -> std::result::Result<(SearchRequest, Filter, u32), String> {
        let model_domains = |domains: &Option<Vec<String>>, field: &str| {
            filter::normalize_domains(domains.as_deref().unwrap_or_default()).map_err(|bad| {
                format!(
                    "{field} entry {bad:?} is not a hostname. Use bare hostnames such as \
                     \"docs.rs\", without scheme, path or port."
                )
            })
        };
        if input
            .country
            .as_deref()
            .is_some_and(|c| c.trim().is_empty())
        {
            return Err("country must not be empty; omit it to use the default.".to_string());
        }
        let requested = model_domains(&input.include_domains, "include_domains")?;
        let excluded = model_domains(&input.exclude_domains, "exclude_domains")?;

        let narrowed: Vec<String> = if filter.allowed.is_empty() {
            requested.clone()
        } else if requested.is_empty() {
            filter.allowed.clone()
        } else {
            requested
                .iter()
                .filter(|d| filter.allowed.iter().any(|a| filter::domain_matches(d, a)))
                .cloned()
                .collect()
        };
        if !requested.is_empty() && narrowed.is_empty() {
            return Err(format!(
                "None of the requested include_domains may be searched. Allowed domains: {}.",
                filter.allowed.join(", ")
            ));
        }
        let unblocked: Vec<String> = narrowed
            .iter()
            .filter(|d| !filter::is_blocked(d, &filter.blocked))
            .cloned()
            .collect();
        if !narrowed.is_empty() && unblocked.is_empty() {
            return Err("Every requested include_domains entry is blocked.".to_string());
        }
        let include: Vec<String> = unblocked
            .iter()
            .filter(|d| !filter::is_blocked(d, &excluded))
            .cloned()
            .collect();
        if !unblocked.is_empty() && include.is_empty() {
            return Err(format!(
                "Your exclude_domains remove every domain that may be searched ({}). Exclude \
                 fewer domains.",
                unblocked.join(", ")
            ));
        }
        filter.blocked.extend(excluded);

        let total = input.limit.unwrap_or(self.default_limit);
        let mut sources = Vec::new();
        let model_sources = input.sources.iter().flatten().map(|s| s.source());
        for source in model_sources {
            if !sources.contains(&source) {
                sources.push(source);
            }
        }
        // Firecrawl's limit applies per source; the surplus is trimmed after the search.
        let per_source = total.div_ceil(u32::try_from(sources.len().max(1)).unwrap_or(1));
        let mut request = SearchRequest::new(input.query.clone())
            .with_limit(per_source)
            .with_sources(sources);
        // Firecrawl takes only one of the two lists; blocked domains under an included one are
        // still dropped from the results afterwards.
        if include.is_empty() {
            request = request.with_exclude_domains(dedup(filter.blocked.clone()));
        } else {
            request = request.with_include_domains(dedup(include));
        }
        if let Some(recency) = input.recency {
            request = request.with_time_range(recency.time_range());
        }
        if let Some(country) = input.country.clone().or_else(|| self.country.clone()) {
            request = request.with_country(country);
        }
        if let Some(location) = &self.location {
            request = request.with_location(location.clone());
        }
        Ok((request, filter, total))
    }

    fn result(
        &self,
        id: String,
        query: &str,
        response: SearchResponse,
        filter: &Filter,
        total: u32,
    ) -> ToolResultBlock {
        let data = response.data.unwrap_or_default();
        let mut blocks = Vec::new();
        for web in data.web.unwrap_or_default() {
            let Some(url) = web.url.filter(|u| filter.permits(u)) else {
                continue;
            };
            let mut content = Vec::new();
            tool_result::push_text(&mut content, web.description);
            tool_result::push_text(
                &mut content,
                web.markdown
                    .map(|m| tool_result::truncate(&m, self.max_content_chars)),
            );
            blocks.push(tool_result::search_result(url, web.title, content));
        }
        for news in data.news.unwrap_or_default() {
            let Some(url) = news.url.filter(|u| filter.permits(u)) else {
                continue;
            };
            let mut content = Vec::new();
            tool_result::push_text(&mut content, news.snippet);
            tool_result::push_text(&mut content, news.date.map(|d| format!("Published: {d}")));
            tool_result::push_text(
                &mut content,
                news.markdown
                    .map(|m| tool_result::truncate(&m, self.max_content_chars)),
            );
            blocks.push(tool_result::search_result(url, news.title, content));
        }
        blocks.truncate(usize::try_from(total).unwrap_or(usize::MAX));
        let warning = response
            .warning
            .filter(|w| !w.trim().is_empty())
            .map(|w| format!("Firecrawl warning: {w}"));
        if blocks.is_empty() {
            let text = match warning {
                Some(warning) => format!("No results for {query}. {warning}"),
                None => format!("No results for {query}"),
            };
            return ToolResultBlock::new(id).with_string_content(text);
        }
        if let Some(warning) = warning {
            blocks.push(Content::Text(TextBlock::new(warning)));
        }
        ToolResultBlock::new(id).with_array_content(blocks)
    }
}

impl ClientTool for FirecrawlWebSearch {
    fn name(&self) -> &str {
        WEB_SEARCH_TOOL_NAME
    }

    fn definition(&self) -> ToolUnionParam {
        self.tool()
    }

    fn call<'a>(&'a self, tool_use: &'a ToolUseBlock) -> BoxFuture<'a, Result<ToolResultBlock>> {
        Box::pin(self.execute(tool_use))
    }
}

/// A sandbox `search(...)` call, before its checks.
#[cfg(feature = "code-mode")]
pub(crate) struct SearchQuery {
    pub(crate) query: String,
    pub(crate) limit: Option<u32>,
    pub(crate) scrape: bool,
    pub(crate) include_domains: Option<Vec<String>>,
    pub(crate) exclude_domains: Option<Vec<String>>,
    pub(crate) time_range: Option<TimeRange>,
    pub(crate) sort_by_date: bool,
    pub(crate) categories: Vec<SearchCategory>,
    pub(crate) country: Option<String>,
}

/// A search that passed every check, ready to send.
#[cfg(feature = "code-mode")]
pub(crate) struct PreparedSearch {
    request: SearchRequest,
    filter: Filter,
    total: u32,
}

#[cfg(feature = "code-mode")]
impl PreparedSearch {
    /// The credits the search costs at most: 2 per 10 results asked for, and 1 for each result
    /// it scrapes.
    pub(crate) fn estimated_credits(&self) -> u64 {
        let total = u64::from(self.total);
        let scraped = if self.request.scrape_options.is_some() {
            total
        } else {
            0
        };
        2 * total.div_ceil(10) + scraped
    }
}

/// What a sandbox search found.
#[cfg(feature = "code-mode")]
pub(crate) struct Searched {
    /// Web results the domain lists permit, each with a URL; markdown cut to the tool's limit.
    pub(crate) results: Vec<WebResult>,
    /// Firecrawl's `creditsUsed` (0 for a cache hit), else 2 per 10 results returned (2 for
    /// none) plus 1 for each scraped one.
    pub(crate) credits: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelInput {
    query: String,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    recency: Option<Recency>,
    #[serde(default)]
    sources: Option<Vec<ModelSource>>,
    #[serde(default)]
    include_domains: Option<Vec<String>>,
    #[serde(default)]
    exclude_domains: Option<Vec<String>>,
    #[serde(default)]
    country: Option<String>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Recency {
    Hour,
    Day,
    Week,
    Month,
    Year,
}

impl Recency {
    fn time_range(self) -> TimeRange {
        match self {
            Self::Hour => TimeRange::PastHour,
            Self::Day => TimeRange::PastDay,
            Self::Week => TimeRange::PastWeek,
            Self::Month => TimeRange::PastMonth,
            Self::Year => TimeRange::PastYear,
        }
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ModelSource {
    Web,
    News,
}

impl ModelSource {
    fn source(self) -> SearchSource {
        match self {
            Self::Web => SearchSource::Web,
            Self::News => SearchSource::News,
        }
    }
}

fn merge_description(base: &serde_json::Value, description: &str) -> serde_json::Value {
    let mut value = base.clone();
    value["description"] = json!(description);
    value
}

fn dedup(mut domains: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    domains.retain(|d| seen.insert(d.clone()));
    domains
}
