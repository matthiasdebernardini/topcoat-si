//! Web search and page fetching through Firecrawl's v2 API.
//!
//! [`FirecrawlClient`] is a typed client for `POST /v2/search`, `POST /v2/search/gov` (the
//! Government Index), `POST /v2/scrape`, `POST /v2/map` and `GET /v2/team/credit-usage`.
//! [`FirecrawlWebSearch`] wraps search as a client-side `web_search` tool: hand
//! [`FirecrawlWebSearch::tool`] to the model and answer its calls with
//! [`FirecrawlWebSearch::execute`]. [`FirecrawlFetchPage`] wraps scrape as a client-side
//! `fetch_page` tool through [`ClientTool`](crate::ClientTool).

mod cache;
mod client;
mod credit_usage;
mod fetch_page;
mod filter;
mod gov_search;
mod map;
mod result_metadata;
mod scrape_options;
mod scrape_request;
mod scrape_response;
mod search_request;
mod search_response;
mod tool_result;
mod web_search;

#[cfg(test)]
mod tests;

pub use cache::FirecrawlCache;
pub use client::FirecrawlClient;
pub use credit_usage::CreditUsage;
pub use fetch_page::FirecrawlFetchPage;
pub use gov_search::{CourtType, GovResult, GovSearchData, GovSearchRequest, GovSearchResponse};
pub use map::{MapLink, MapLocation, MapRequest, MapResponse, SitemapMode};
pub use result_metadata::ResultMetadata;
pub use scrape_options::{PdfMode, ScrapeOptions};
pub use scrape_request::ScrapeRequest;
pub use scrape_response::{PdfPage, ScrapeData, ScrapeResponse};
pub use search_request::{SearchCategory, SearchRequest, SearchSource, TimeRange};
pub use search_response::{ImageResult, NewsResult, SearchData, SearchResponse, WebResult};
#[cfg(feature = "code-mode")]
pub(crate) use web_search::SearchQuery;
pub use web_search::{FirecrawlWebSearch, WEB_SEARCH_TOOL_NAME};
