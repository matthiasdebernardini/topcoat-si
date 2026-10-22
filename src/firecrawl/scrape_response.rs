//! The scrape response.

use serde::{Deserialize, Serialize};

use super::ResultMetadata;

/// A successful Firecrawl scrape.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(from = "WireScrapeResponse")]
#[non_exhaustive]
pub struct ScrapeResponse {
    /// Whether the scrape succeeded.
    pub success: bool,
    /// The scraped page. Empty when Firecrawl sent no data.
    pub data: ScrapeData,
}

/// A scraped page.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[non_exhaustive]
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
