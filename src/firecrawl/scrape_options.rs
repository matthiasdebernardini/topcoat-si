//! How pages are scraped.

use std::time::Duration;

use serde::Serialize;

use crate::error::{Error, Result};
use crate::types::PdfTag;

const MIN_SCRAPE_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_SCRAPE_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_PDF_PAGES: u32 = 10_000;

// Formats Firecrawl bills 4 extra credits per page for. This crate requests none of them yet.
const PAID_FORMATS: [&str; 6] = ["json", "question", "query", "highlights", "audio", "video"];

/// How a PDF's text is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum PdfMode {
    /// Read only the embedded text. Fastest, but fails on scanned or image-only pages.
    Fast,
    /// Read the embedded text and fall back to OCR for pages that need it. Firecrawl's default.
    Auto,
    /// OCR every page, for scanned documents.
    Ocr,
}

/// How a page is scraped as markdown, for a search result or a single scrape.
///
/// Every scraped search result costs one extra Firecrawl credit. A PDF costs one credit per
/// page read, so cap long documents with [`ScrapeOptions::with_pdf_max_pages`].
///
/// Any PDF option (`pdf_max_pages`, `pdf_mode`, `pdf_pages`, `pdf_page_markers`) sends a parsers
/// list holding the configured PDF parser and the image parser, so image OCR stays on as in
/// Firecrawl's default.
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
    /// Firecrawl bills 1 credit per PDF page.
    pub pdf_max_pages: Option<u32>,
    /// How PDF text is read. Firecrawl defaults to [`PdfMode::Auto`]. Costs nothing extra.
    pub pdf_mode: Option<PdfMode>,
    /// Also return each PDF page's markdown in [`ScrapeData::pages`](super::ScrapeData::pages).
    /// Costs nothing extra. Only a [`FirecrawlClient::scrape`](super::FirecrawlClient::scrape)
    /// answer carries the pages; a search result has no field for them.
    pub pdf_pages: Option<bool>,
    /// Join PDF pages in the markdown with `<!-- page N -->` markers, N being the 1-based page
    /// that follows. There is no marker before page 1. Costs nothing extra.
    pub pdf_page_markers: Option<bool>,
    /// Also return the page's links in [`ScrapeData::links`](super::ScrapeData::links). Costs
    /// nothing extra. Only a [`FirecrawlClient::scrape`](super::FirecrawlClient::scrape) answer
    /// carries the links; a search result has no field for them.
    pub links: bool,
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

    /// Read at most this many PDF pages.
    #[must_use]
    pub fn with_pdf_max_pages(mut self, pages: u32) -> Self {
        self.pdf_max_pages = Some(pages);
        self
    }

    /// Set how PDF text is read.
    #[must_use]
    pub fn with_pdf_mode(mut self, mode: PdfMode) -> Self {
        self.pdf_mode = Some(mode);
        self
    }

    /// Return each PDF page's markdown separately, on a scrape only.
    #[must_use]
    pub fn with_pdf_pages(mut self, pages: bool) -> Self {
        self.pdf_pages = Some(pages);
        self
    }

    /// Mark PDF page breaks in the markdown.
    #[must_use]
    pub fn with_pdf_page_markers(mut self, page_markers: bool) -> Self {
        self.pdf_page_markers = Some(page_markers);
        self
    }

    /// Also return the links on the page, on a scrape only.
    #[must_use]
    pub fn with_links(mut self, links: bool) -> Self {
        self.links = links;
        self
    }

    /// How many of the requested formats Firecrawl bills 4 extra credits per page for.
    pub(super) fn paid_formats(&self) -> u64 {
        let paid = self
            .formats()
            .iter()
            .filter(|format| PAID_FORMATS.contains(format))
            .count();
        u64::try_from(paid).unwrap_or(u64::MAX)
    }

    fn formats(&self) -> Vec<&'static str> {
        let mut formats = vec!["markdown"];
        if self.links {
            formats.push("links");
        }
        formats
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
            formats: self.formats(),
            only_main_content: self.only_main_content,
            max_age: self.max_age.map(millis),
            timeout: self.timeout.map(millis),
            // A parsers list switches off every parser it omits, so the image parser is listed
            // too.
            parsers: self
                .pdf_parser()
                .map(|pdf| vec![WireParserEntry::Pdf(pdf), WireParserEntry::Named("image")]),
        }
    }

    /// The PDF parser entry, when any PDF option is set.
    fn pdf_parser(&self) -> Option<WireParser> {
        let set = self.pdf_max_pages.is_some()
            || self.pdf_mode.is_some()
            || self.pdf_pages.is_some()
            || self.pdf_page_markers.is_some();
        set.then_some(WireParser {
            kind: PdfTag,
            mode: self.pdf_mode,
            max_pages: self.pdf_max_pages,
            pages: self.pdf_pages,
            page_markers: self.pdf_page_markers,
        })
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
    parsers: Option<Vec<WireParserEntry>>,
}

/// One entry of the parsers list: a configured parser, or a parser's name for its defaults.
#[derive(Serialize)]
#[serde(untagged)]
pub(super) enum WireParserEntry {
    Pdf(WireParser),
    Named(&'static str),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WireParser {
    #[serde(rename = "type")]
    kind: PdfTag,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<PdfMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_pages: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pages: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    page_markers: Option<bool>,
}
