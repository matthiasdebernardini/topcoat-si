//! How pages are scraped.

use std::time::Duration;

use serde::Serialize;

use crate::error::{Error, Result};
use crate::types::PdfTag;

const MIN_SCRAPE_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_SCRAPE_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_PDF_PAGES: u32 = 10_000;

/// How a page is scraped as markdown, for a search result or a single scrape.
///
/// Every scraped search result costs one extra Firecrawl credit. A PDF costs one credit per
/// page read, so cap long documents with [`ScrapeOptions::with_pdf_max_pages`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
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
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Keep only the main content of each page.
    #[must_use]
    pub fn with_only_main_content(mut self, only_main_content: bool) -> Self {
        self.only_main_content = Some(only_main_content);
        self
    }

    /// Accept cached pages up to this age.
    #[must_use]
    pub fn with_max_age(mut self, max_age: Duration) -> Self {
        self.max_age = Some(max_age);
        self
    }

    /// Set the server-side time limit for the scrape.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Read at most this many PDF pages. Also switches off image OCR, see
    /// [`ScrapeOptions::pdf_max_pages`].
    #[must_use]
    pub fn with_pdf_max_pages(mut self, pages: u32) -> Self {
        self.pdf_max_pages = Some(pages);
        self
    }

    pub(super) fn validate(&self) -> Result<()> {
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

    pub(super) fn wire(&self) -> WireScrapeOptions {
        WireScrapeOptions {
            formats: vec!["markdown"],
            only_main_content: self.only_main_content,
            max_age: self.max_age.map(millis),
            timeout: self.timeout.map(millis),
            parsers: self.pdf_max_pages.map(|max_pages| {
                vec![WireParser {
                    kind: PdfTag,
                    max_pages,
                }]
            }),
        }
    }
}

pub(super) fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WireScrapeOptions {
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
pub(super) struct WireParser {
    #[serde(rename = "type")]
    kind: PdfTag,
    max_pages: u32,
}
