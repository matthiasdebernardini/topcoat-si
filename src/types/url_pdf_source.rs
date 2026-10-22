use serde::{Deserialize, Serialize};

/// A source for a PDF from a URL.
///
/// This type is used to provide a PDF to the model from a URL.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct UrlPdfSource {
    /// The URL of the PDF.
    pub url: String,
}

impl UrlPdfSource {
    /// Creates a new UrlPdfSource with the specified URL.
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialization() {
        let source = UrlPdfSource {
            url: "https://example.com/document.pdf".to_string(),
        };

        let json = serde_json::to_value(&source).unwrap();
        let expected = serde_json::json!({"url": "https://example.com/document.pdf"});

        assert_eq!(json, expected);
    }

    #[test]
    fn deserialization() {
        let json = serde_json::json!({"url": "https://example.com/document.pdf"});
        let source: UrlPdfSource = serde_json::from_value(json).unwrap();

        assert_eq!(source.url, "https://example.com/document.pdf");
    }
}
