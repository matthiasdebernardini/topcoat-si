use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::types::open_union;
use crate::types::{
    CitationCharLocation, CitationContentBlockLocation, CitationPageLocation,
    CitationSearchResultLocation,
};

/// Represents a citation object that could be any of the supported citation types
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum Citation {
    /// Citation referencing a character location in source text.
    #[serde(rename = "char_location")]
    CharLocation(CitationCharLocation),

    /// Citation referencing a page location in a document.
    #[serde(rename = "page_location")]
    PageLocation(CitationPageLocation),

    /// Citation referencing a content block location.
    #[serde(rename = "content_block_location")]
    ContentBlockLocation(CitationContentBlockLocation),

    /// Citation referencing a search result location.
    #[serde(rename = "search_result_location")]
    SearchResultLocation(CitationSearchResultLocation),

    /// A citation of a type this crate does not know, kept verbatim.
    #[serde(untagged)]
    Unknown(Value),
}

impl<'de> Deserialize<'de> for Citation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        match open_union::type_tag::<D::Error>(&value)?.as_str() {
            "char_location" => open_union::from_value(value).map(Citation::CharLocation),
            "page_location" => open_union::from_value(value).map(Citation::PageLocation),
            "content_block_location" => {
                open_union::from_value(value).map(Citation::ContentBlockLocation)
            }
            "search_result_location" => {
                open_union::from_value(value).map(Citation::SearchResultLocation)
            }
            tag => {
                open_union::warn_unknown("Citation", tag);
                Ok(Citation::Unknown(value))
            }
        }
    }
}
