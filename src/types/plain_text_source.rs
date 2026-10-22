use serde::{Deserialize, Serialize};

/// A plain text source parameter for content blocks.
///
/// This represents plain text data that can be used as a source in content blocks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct PlainTextSource {
    /// The plain text data.
    pub data: String,

    /// The media type, which is always "text/plain".
    #[serde(rename = "media_type")]
    pub media_type: String,
}

impl PlainTextSource {
    /// Create a new `PlainTextSource` with the given text data.
    #[must_use]
    pub fn new(data: impl Into<String>) -> Self {
        Self {
            data: data.into(),
            media_type: "text/plain".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

    #[test]
    fn plain_text_source_serialization() {
        let source = PlainTextSource::new("Sample text content");
        let json = to_value(&source).unwrap();

        assert_eq!(
            json,
            json!({
                "data": "Sample text content",
                "media_type": "text/plain"
            })
        );
    }

    #[test]
    fn plain_text_source_from_literal() {
        let source = PlainTextSource::new("Sample text content");
        assert_eq!(source.data, "Sample text content");
        assert_eq!(source.media_type, "text/plain");
    }
}
