//! The search response.

use serde::{Deserialize, Serialize};

use super::ResultMetadata;

/// A successful Firecrawl search.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
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
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[non_exhaustive]
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
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[non_exhaustive]
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
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[non_exhaustive]
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
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
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
