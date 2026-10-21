//! Scrape parameters.

use serde::Serialize;

use super::ScrapeOptions;
use super::scrape_options::WireScrapeOptions;
use crate::error::{Error, Result};

/// Parameters for [`FirecrawlClient::scrape`](super::FirecrawlClient::scrape).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ScrapeRequest {
    /// Absolute http or https URL of the page.
    pub url: String,
    /// How the page is scraped. Markdown is always requested.
    pub options: ScrapeOptions,
}

impl ScrapeRequest {
    /// Create a request for `url` with Firecrawl's defaults.
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            options: ScrapeOptions::new(),
        }
    }

    /// Set how the page is scraped.
    #[must_use]
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

    pub(super) fn wire(&self) -> WireScrapeRequest<'_> {
        WireScrapeRequest {
            url: self.url.trim(),
            options: self.options.wire(),
        }
    }
}

#[derive(Serialize)]
pub(super) struct WireScrapeRequest<'request> {
    url: &'request str,
    #[serde(flatten)]
    options: WireScrapeOptions,
}
