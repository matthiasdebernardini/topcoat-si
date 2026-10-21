use serde::{Deserialize, Serialize};

/// A citation of a span of blocks inside a `search_result` content block.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct CitationSearchResultLocation {
    /// The text that was cited
    pub cited_text: String,

    /// The index of the search result in the input context
    pub search_result_index: i32,

    /// The source of the search result
    pub source: String,

    /// Title of the search result
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// The start block index (inclusive) of the citation in the search result
    pub start_block_index: i32,

    /// The end block index (exclusive) of the citation in the search result
    pub end_block_index: i32,
}

impl CitationSearchResultLocation {
    /// Creates a new CitationSearchResultLocation
    #[must_use]
    pub fn new(
        cited_text: impl Into<String>,
        search_result_index: i32,
        source: impl Into<String>,
        start_block_index: i32,
        end_block_index: i32,
        title: Option<String>,
    ) -> Self {
        Self {
            cited_text: cited_text.into(),
            search_result_index,
            source: source.into(),
            title,
            start_block_index,
            end_block_index,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_serializes_like_the_api() {
        let location = CitationSearchResultLocation::new(
            "example text",
            2,
            "https://example.com/a",
            0,
            3,
            Some("Result Title".to_string()),
        );

        let json = serde_json::to_value(&location).unwrap();

        assert_eq!(
            json,
            serde_json::json!({
                "cited_text": "example text",
                "search_result_index": 2,
                "source": "https://example.com/a",
                "title": "Result Title",
                "start_block_index": 0,
                "end_block_index": 3
            })
        );
    }
}
