use serde::{Deserialize, Serialize, Serializer};

use crate::types::open_union;
use crate::types::{CacheControlEphemeral, CitationsConfig, TextBlock};

fn serialize_content<S>(content: &[TextBlock], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    open_union::with_type_seq("text", content, serializer)
}

/// A search result supplied as input, enabling RAG-style citations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct SearchResultBlock {
    /// Where the result came from, such as a URL or document identifier.
    pub source: String,

    /// Title of the result.
    pub title: String,

    /// The text content of the result.
    #[serde(serialize_with = "serialize_content")]
    pub content: Vec<TextBlock>,

    /// Citation configuration for this result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citations: Option<CitationsConfig>,

    /// Create a cache control breakpoint at this content block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControlEphemeral>,
}

impl SearchResultBlock {
    /// Creates a search result block.
    #[must_use]
    pub fn new(
        source: impl Into<String>,
        title: impl Into<String>,
        content: Vec<TextBlock>,
    ) -> Self {
        Self {
            source: source.into(),
            title: title.into(),
            content,
            citations: None,
            cache_control: None,
        }
    }

    /// Sets the citation configuration.
    #[must_use]
    pub fn with_citations(mut self, citations: CitationsConfig) -> Self {
        self.citations = Some(citations);
        self
    }

    /// Sets the cache control.
    #[must_use]
    pub fn with_cache_control(mut self, cache_control: CacheControlEphemeral) -> Self {
        self.cache_control = Some(cache_control);
        self
    }
}
