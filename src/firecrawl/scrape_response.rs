//! The scrape response.

use serde::{Deserialize, Serialize};

use super::{ResultMetadata, ScrapeOptions};

// Firecrawl's base charge for a scraped page.
const BASE_CREDITS: u64 = 1;
// Firecrawl's charge per page for each paid add-on format.
const PAID_FORMAT_CREDITS: u64 = 4;

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
    /// Links on the page, when [`ScrapeOptions::with_links`] asked for them.
    #[serde(default)]
    pub links: Option<Vec<String>>,
    /// Each PDF page's markdown, when [`ScrapeOptions::with_pdf_pages`] asked for it.
    #[serde(default)]
    pub pages: Option<Vec<PdfPage>>,
}

/// One physical page of a scraped PDF.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[non_exhaustive]
pub struct PdfPage {
    /// The 1-based physical page number, read from `pageNumber`. 0 when Firecrawl sent none.
    #[serde(rename = "pageNumber", default)]
    pub page: u32,
    /// The page's content as markdown.
    #[serde(default)]
    pub markdown: String,
}

impl ScrapeData {
    /// The Firecrawl credits this scrape cost, given the `options` it was requested with.
    ///
    /// Firecrawl's own figure, [`ResultMetadata::credits_used`], wins when the response carries
    /// one. A scrape served from [`FirecrawlClient::with_cache`](super::FirecrawlClient::with_cache)
    /// always reports 0 there, because a local cache hit bills nothing.
    ///
    /// Otherwise the figure is estimated from Firecrawl's billing: 1 credit for the page, plus 1
    /// for each PDF page read ([`ResultMetadata::num_pages`]) after the first, plus 4 for each paid
    /// add-on format (`json`, `question`, `query`, `highlights`, `audio`, `video`), none of which
    /// [`ScrapeOptions`] requests. Links, per-page markdown, page markers and the PDF mode cost
    /// nothing extra. The estimate leaves out what this crate does not request: zero data
    /// retention (+1), PII redaction, lockdown and prompt-injection checks (+4 each), and
    /// third-party provider pricing. Firecrawl's billing code charges `numPages - 1` extra PDF
    /// pages on top of the base credit, so a PDF costs 1 credit per page: 40 for 40 pages, 1 for
    /// one. A page Firecrawl serves from its own cache (see [`ScrapeOptions::max_age`]) is still
    /// billed.
    #[must_use]
    pub fn credits(&self, options: &ScrapeOptions) -> u64 {
        let metadata = self.metadata.as_ref();
        if let Some(credits) = metadata.and_then(|metadata| metadata.credits_used) {
            return credits;
        }
        let pdf_pages = metadata
            .and_then(|metadata| metadata.num_pages)
            .map_or(0, u64::from);
        BASE_CREDITS + pdf_pages.saturating_sub(1) + PAID_FORMAT_CREDITS * options.paid_formats()
    }
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
