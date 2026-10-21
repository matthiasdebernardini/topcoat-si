use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::types::open_union;
use crate::types::{
    Base64PdfSource, CacheControlEphemeral, CitationsConfig, ContentBlockSourceParam, FileSource,
    PlainTextSource, UrlPdfSource,
};

/// The source type for a document block, which can be one of several types.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum DocumentSource {
    /// A Base64 encoded PDF source.
    #[serde(rename = "base64")]
    Base64Pdf(Base64PdfSource),

    /// A plain text source.
    #[serde(rename = "text")]
    PlainText(PlainTextSource),

    /// A content block source.
    #[serde(rename = "content")]
    ContentBlock(ContentBlockSourceParam),

    /// A URL PDF source.
    #[serde(rename = "url")]
    UrlPdf(UrlPdfSource),

    /// A file uploaded through the Files API.
    #[serde(rename = "file")]
    File(FileSource),

    /// A source of a type this crate does not know, kept verbatim.
    #[serde(untagged)]
    Unknown(Value),
}

impl<'de> Deserialize<'de> for DocumentSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        match open_union::type_tag::<D::Error>(&value)?.as_str() {
            "base64" => open_union::from_value(value).map(Self::Base64Pdf),
            "text" => open_union::from_value(value).map(Self::PlainText),
            "content" => open_union::from_value(value).map(Self::ContentBlock),
            "url" => open_union::from_value(value).map(Self::UrlPdf),
            "file" => open_union::from_value(value).map(Self::File),
            tag => {
                open_union::warn_unknown("DocumentSource", tag);
                Ok(Self::Unknown(value))
            }
        }
    }
}

impl From<Base64PdfSource> for DocumentSource {
    fn from(source: Base64PdfSource) -> Self {
        DocumentSource::Base64Pdf(source)
    }
}

impl From<PlainTextSource> for DocumentSource {
    fn from(source: PlainTextSource) -> Self {
        DocumentSource::PlainText(source)
    }
}

impl From<ContentBlockSourceParam> for DocumentSource {
    fn from(source: ContentBlockSourceParam) -> Self {
        DocumentSource::ContentBlock(source)
    }
}

impl From<FileSource> for DocumentSource {
    fn from(source: FileSource) -> Self {
        DocumentSource::File(source)
    }
}

impl From<UrlPdfSource> for DocumentSource {
    fn from(source: UrlPdfSource) -> Self {
        DocumentSource::UrlPdf(source)
    }
}

/// Parameters for a document block.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct DocumentBlock {
    /// The source of the document.
    pub source: DocumentSource,

    /// Create a cache control breakpoint at this content block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControlEphemeral>,

    /// Configuration for citations in this document.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub citations: Option<CitationsConfig>,

    /// Optional context for the document.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,

    /// Optional title for the document.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl DocumentBlock {
    /// Create a new `DocumentBlock` with the given source.
    #[must_use]
    pub fn new(source: DocumentSource) -> Self {
        Self {
            source,
            cache_control: None,
            citations: None,
            context: None,
            title: None,
        }
    }

    /// Create a new `DocumentBlock` referencing a file uploaded through the Files API.
    #[must_use]
    pub fn from_file_id(file_id: impl Into<String>) -> Self {
        Self::new(DocumentSource::File(FileSource::new(file_id)))
    }

    /// Add a cache control to this document block.
    #[must_use]
    pub fn with_cache_control(mut self, cache_control: CacheControlEphemeral) -> Self {
        self.cache_control = Some(cache_control);
        self
    }

    /// Add citations configuration to this document block.
    #[must_use]
    pub fn with_citations(mut self, citations: CitationsConfig) -> Self {
        self.citations = Some(citations);
        self
    }

    /// Add context to this document block.
    #[must_use]
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }

    /// Add a title to this document block.
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

    #[test]
    fn document_block_with_base64_pdf() {
        let base64_source =
            Base64PdfSource::new("data:application/pdf;base64,JVBERi0xLjcKJeLjz9MKN");

        let document_block = DocumentBlock::new(base64_source.into());
        let json = to_value(&document_block).unwrap();

        assert_eq!(
            json,
            json!({
                "source": {
                    "type": "base64",
                    "data": "data:application/pdf;base64,JVBERi0xLjcKJeLjz9MKN",
                    "media_type": "application/pdf"
                }
            })
        );
    }

    #[test]
    fn document_block_with_plain_text() {
        let text_source = PlainTextSource::new("Sample text content");

        let document_block = DocumentBlock::new(text_source.into());
        let json = to_value(&document_block).unwrap();

        assert_eq!(
            json,
            json!({
                "source": {
                    "type": "text",
                    "data": "Sample text content",
                    "media_type": "text/plain"
                }
            })
        );
    }

    #[test]
    fn document_block_with_content_block() {
        let content_source = ContentBlockSourceParam::new_with_string("Sample content");

        let document_block = DocumentBlock::new(content_source.into());
        let json = to_value(&document_block).unwrap();

        assert_eq!(
            json,
            json!({
                "source": {
                    "type": "content",
                    "content": "Sample content"
                }
            })
        );
    }

    #[test]
    fn document_block_with_url_pdf() {
        let url_source = UrlPdfSource::new("https://example.com/document.pdf");

        let document_block = DocumentBlock::new(url_source.into());
        let json = to_value(&document_block).unwrap();

        assert_eq!(
            json,
            json!({
                "source": {
                    "type": "url",
                    "url": "https://example.com/document.pdf"
                }
            })
        );
    }

    #[test]
    fn document_block_with_all_fields() {
        let url_source = UrlPdfSource::new("https://example.com/document.pdf");
        let cache_control = CacheControlEphemeral::new();
        let citations = CitationsConfig::enabled();

        let document_block = DocumentBlock::new(url_source.into())
            .with_cache_control(cache_control)
            .with_citations(citations)
            .with_context("Document context")
            .with_title("Document Title");

        let json = to_value(&document_block).unwrap();

        assert_eq!(
            json,
            json!({
                "source": {
                    "type": "url",
                    "url": "https://example.com/document.pdf"
                },
                "cache_control": {
                    "type": "ephemeral"
                },
                "citations": {
                    "enabled": true
                },
                "context": "Document context",
                "title": "Document Title"
            })
        );
    }

    #[test]
    fn document_block_with_file_source() {
        let block = DocumentBlock::from_file_id("file_01");
        let json = to_value(&block).unwrap();
        assert_eq!(
            json,
            json!({"source": {"type": "file", "file_id": "file_01"}})
        );
        let parsed: DocumentBlock = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, block);
    }
}
