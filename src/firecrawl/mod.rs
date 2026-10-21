//! Web search and page fetching through Firecrawl's v2 API.
//!
//! [`FirecrawlClient`] is a typed client for `POST /v2/search` and `POST /v2/scrape`.
//! [`FirecrawlWebSearch`] wraps search as a client-side `web_search` tool: hand
//! [`FirecrawlWebSearch::tool`] to the model and answer its calls with
//! [`FirecrawlWebSearch::execute`]. [`FirecrawlFetchPage`] wraps scrape as a client-side
//! `fetch_page` tool through [`ClientTool`](crate::ClientTool).

mod client;
mod fetch_page;
mod filter;
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

pub use client::FirecrawlClient;
pub use fetch_page::FirecrawlFetchPage;
pub use result_metadata::ResultMetadata;
pub use scrape_options::ScrapeOptions;
pub use scrape_request::ScrapeRequest;
pub use scrape_response::{ScrapeData, ScrapeResponse};
pub use search_request::{SearchCategory, SearchRequest, SearchSource, TimeRange};
pub use search_response::{ImageResult, NewsResult, SearchData, SearchResponse, WebResult};
pub use web_search::{FirecrawlWebSearch, WEB_SEARCH_TOOL_NAME};
