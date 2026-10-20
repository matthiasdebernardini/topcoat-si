use serde::{Deserialize, Serialize};

/// A citation of a span of blocks inside a `search_result` content block.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
