//! Metadata of a scraped page.

use serde::{Deserialize, Deserializer};

/// Metadata from scraping a page.
///
/// Parsing never fails: a field that is missing or of an unexpected type is `None`. Title and
/// description may arrive as an array of strings, in which case the first is kept.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct ResultMetadata {
    /// Page title.
    pub title: Option<String>,
    /// Page description.
    pub description: Option<String>,
    /// URL that was requested.
    pub source_url: Option<String>,
    /// URL after redirects.
    pub url: Option<String>,
    /// HTTP status of the scraped page.
    pub status_code: Option<u16>,
    /// Scrape error for this page.
    pub error: Option<String>,
    /// MIME type of the page, such as `text/html` or `application/pdf`.
    pub content_type: Option<String>,
    /// For a PDF, the number of pages read (capped by the PDF page limit).
    pub num_pages: Option<u32>,
    /// For a PDF, the document's full page count, when Firecrawl knows it.
    pub total_pages: Option<u32>,
}

impl<'de> Deserialize<'de> for ResultMetadata {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let text = |name: &str| value.get(name).and_then(first_string);
        let number = |name: &str| value.get(name).and_then(serde_json::Value::as_u64);
        Ok(Self {
            title: text("title"),
            description: text("description"),
            source_url: text("sourceURL"),
            url: text("url"),
            status_code: number("statusCode").and_then(|n| u16::try_from(n).ok()),
            error: text("error"),
            content_type: text("contentType"),
            num_pages: number("numPages").and_then(|n| u32::try_from(n).ok()),
            total_pages: number("totalPages").and_then(|n| u32::try_from(n).ok()),
        })
    }
}

/// A string, or the first element of an array of strings.
fn first_string(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Array(items) => items.first().and_then(first_string),
        _ => None,
    }
}
