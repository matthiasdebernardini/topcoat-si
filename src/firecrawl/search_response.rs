//! The search response.

use serde::{Deserialize, Deserializer, Serialize};

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
    /// Results grouped by source. A flat `data` array of results is read as web results.
    #[serde(default, deserialize_with = "grouped_or_flat")]
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
    /// The passage from the page that best answers the query, or the site's own description
    /// when the search was sent with
    /// [`SearchRequest::with_highlights(false)`](super::SearchRequest::with_highlights).
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
    /// The passage from the article that best answers the query, or the site's own snippet
    /// when the search was sent with
    /// [`SearchRequest::with_highlights(false)`](super::SearchRequest::with_highlights).
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

/// The results as an object of arrays by source, or as one flat array of web results.
fn grouped_or_flat<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<SearchData>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Wire {
        // Tried first: a struct also accepts an array, by position.
        Flat(Vec<WebResult>),
        Grouped(SearchData),
    }
    Ok(
        Option::<Wire>::deserialize(deserializer)?.map(|wire| match wire {
            Wire::Flat(web) => SearchData {
                web: Some(web),
                ..SearchData::default()
            },
            Wire::Grouped(data) => data,
        }),
    )
}
