use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::types::open_union::{from_value, type_tag};
use crate::types::{
    CitationCharLocation, CitationContentBlockLocation, CitationPageLocation,
    CitationSearchResultLocation,
};

/// A citation reference in a TextBlock.
///
/// This enum represents the different types of citations that can be included
/// in a text block's content.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type")]
pub enum TextCitation {
    /// A character-based location citation
    #[serde(rename = "char_location")]
    CharLocation(CitationCharLocation),

    /// A page-based location citation
    #[serde(rename = "page_location")]
    PageLocation(CitationPageLocation),

    /// A content block-based location citation
    #[serde(rename = "content_block_location")]
    ContentBlockLocation(CitationContentBlockLocation),

    /// A search result location citation
    #[serde(rename = "search_result_location")]
    SearchResultLocation(CitationSearchResultLocation),

    /// A citation of a type this crate does not know, kept verbatim.
    #[serde(untagged)]
    Unknown(Value),
}

impl<'de> Deserialize<'de> for TextCitation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        match type_tag::<D::Error>(&value)?.as_str() {
            "char_location" => from_value(value).map(TextCitation::CharLocation),
            "page_location" => from_value(value).map(TextCitation::PageLocation),
            "content_block_location" => from_value(value).map(TextCitation::ContentBlockLocation),
            "search_result_location" => from_value(value).map(TextCitation::SearchResultLocation),
            _ => Ok(TextCitation::Unknown(value)),
        }
    }
}

impl TextCitation {
    /// Creates a new character-based location citation
    pub fn char_location(
        cited_text: String,
        document_index: i32,
        start_char_index: i32,
        end_char_index: i32,
        document_title: Option<String>,
    ) -> Self {
        let char_location = CitationCharLocation {
            cited_text,
            document_index,
            document_title,
            end_char_index,
            start_char_index,
            file_id: None,
        };
        Self::CharLocation(char_location)
    }

    /// Creates a new page-based location citation
    pub fn page_location(
        cited_text: String,
        document_index: i32,
        start_page_number: i32,
        end_page_number: i32,
        document_title: Option<String>,
    ) -> Self {
        let page_location = CitationPageLocation {
            cited_text,
            document_index,
            document_title,
            end_page_number,
            start_page_number,
            file_id: None,
        };
        Self::PageLocation(page_location)
    }

    /// Creates a new content block-based location citation
    pub fn content_block_location(
        cited_text: String,
        document_index: i32,
        start_block_index: i32,
        end_block_index: i32,
        document_title: Option<String>,
    ) -> Self {
        let content_block_location = CitationContentBlockLocation {
            cited_text,
            document_index,
            document_title,
            end_block_index,
            start_block_index,
            file_id: None,
        };
        Self::ContentBlockLocation(content_block_location)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn char_location_serialization() {
        let char_location = CitationCharLocation {
            cited_text: "example text".to_string(),
            document_index: 0,
            document_title: Some("Document Title".to_string()),
            end_char_index: 12,
            start_char_index: 0,
            file_id: None,
        };

        let citation = TextCitation::CharLocation(char_location);

        let json = serde_json::to_value(&citation).unwrap();
        let expected = serde_json::json!({
            "type": "char_location",
            "cited_text": "example text",
            "document_index": 0,
            "document_title": "Document Title",
            "end_char_index": 12,
            "start_char_index": 0
        });

        assert_eq!(json, expected);
    }

    #[test]
    fn page_location_serialization() {
        let page_location = CitationPageLocation {
            cited_text: "example text".to_string(),
            document_index: 0,
            document_title: Some("Document Title".to_string()),
            end_page_number: 5,
            start_page_number: 3,
            file_id: None,
        };

        let citation = TextCitation::PageLocation(page_location);

        let json = serde_json::to_value(&citation).unwrap();
        let expected = serde_json::json!({
            "type": "page_location",
            "cited_text": "example text",
            "document_index": 0,
            "document_title": "Document Title",
            "end_page_number": 5,
            "start_page_number": 3
        });

        assert_eq!(json, expected);
    }

    #[test]
    fn content_block_location_serialization() {
        let content_block_location = CitationContentBlockLocation {
            cited_text: "example text".to_string(),
            document_index: 0,
            document_title: Some("Document Title".to_string()),
            start_block_index: 1,
            end_block_index: 5,
            file_id: None,
        };

        let citation = TextCitation::ContentBlockLocation(content_block_location);

        let json = serde_json::to_value(&citation).unwrap();
        let expected = serde_json::json!({
            "type": "content_block_location",
            "cited_text": "example text",
            "document_index": 0,
            "document_title": "Document Title",
            "end_block_index": 5,
            "start_block_index": 1
        });

        assert_eq!(json, expected);
    }

    #[test]
    fn deserialization() {
        let json = serde_json::json!({
            "type": "char_location",
            "cited_text": "example text",
            "document_index": 0,
            "document_title": "Document Title",
            "end_char_index": 12,
            "start_char_index": 0
        });
        let citation: TextCitation = serde_json::from_value(json).unwrap();

        match citation {
            TextCitation::CharLocation(location) => {
                assert_eq!(location.cited_text, "example text");
                assert_eq!(location.document_index, 0);
                assert_eq!(location.document_title, Some("Document Title".to_string()));
                assert_eq!(location.end_char_index, 12);
                assert_eq!(location.start_char_index, 0);
            }
            _ => panic!("Expected CharLocation"),
        }
    }

    #[test]
    fn char_location_method() {
        let citation = TextCitation::char_location(
            "example text".to_string(),
            0,
            0,
            12,
            Some("Document Title".to_string()),
        );

        let json_value = serde_json::to_value(&citation).unwrap();
        let expected = json!({
            "type": "char_location",
            "cited_text": "example text",
            "document_index": 0,
            "document_title": "Document Title",
            "end_char_index": 12,
            "start_char_index": 0
        });

        assert_eq!(json_value, expected);
    }

    #[test]
    fn page_location_method() {
        let citation = TextCitation::page_location(
            "example text".to_string(),
            0,
            3,
            5,
            Some("Document Title".to_string()),
        );

        let json_value = serde_json::to_value(&citation).unwrap();
        let expected = json!({
            "type": "page_location",
            "cited_text": "example text",
            "document_index": 0,
            "document_title": "Document Title",
            "end_page_number": 5,
            "start_page_number": 3
        });

        assert_eq!(json_value, expected);
    }

    #[test]
    fn content_block_location_method() {
        let citation = TextCitation::content_block_location(
            "example text".to_string(),
            0,
            2,
            4,
            Some("Document Title".to_string()),
        );

        let json_value = serde_json::to_value(&citation).unwrap();
        let expected = json!({
            "type": "content_block_location",
            "cited_text": "example text",
            "document_index": 0,
            "document_title": "Document Title",
            "end_block_index": 4,
            "start_block_index": 2
        });

        assert_eq!(json_value, expected);
    }

    #[test]
    fn optional_fields_are_omitted() {
        let citation = TextCitation::char_location("example text".to_string(), 0, 0, 12, None);

        let json_value = serde_json::to_value(&citation).unwrap();
        let expected = json!({
            "type": "char_location",
            "cited_text": "example text",
            "document_index": 0,
            "end_char_index": 12,
            "start_char_index": 0
        });

        assert_eq!(json_value, expected);
    }

    #[test]
    fn search_result_location_round_trip() {
        let json = json!({
            "type": "search_result_location",
            "cited_text": "text",
            "search_result_index": 0,
            "source": "https://example.com",
            "title": "Example",
            "start_block_index": 1,
            "end_block_index": 2
        });
        let citation: TextCitation = serde_json::from_value(json.clone()).unwrap();
        assert!(matches!(citation, TextCitation::SearchResultLocation(_)));
        assert_eq!(serde_json::to_value(&citation).unwrap(), json);
    }

    #[test]
    fn char_location_with_file_id() {
        let json = json!({
            "type": "char_location",
            "cited_text": "t",
            "document_index": 0,
            "end_char_index": 1,
            "start_char_index": 0,
            "file_id": "file_01"
        });
        let citation: TextCitation = serde_json::from_value(json.clone()).unwrap();
        let TextCitation::CharLocation(location) = &citation else {
            panic!("expected char_location");
        };
        assert_eq!(location.file_id.as_deref(), Some("file_01"));
        assert_eq!(serde_json::to_value(&citation).unwrap(), json);
    }

    #[test]
    fn unknown_citation_round_trips() {
        let raw = r#"{"type":"future_location","a":{"b":[1,2]}}"#;
        let citation: TextCitation = serde_json::from_str(raw).unwrap();
        assert!(matches!(citation, TextCitation::Unknown(_)));
        assert_eq!(serde_json::to_string(&citation).unwrap(), raw);
    }

    #[test]
    fn malformed_known_citation_errors() {
        let json = json!({"type": "char_location"});
        assert!(serde_json::from_value::<TextCitation>(json).is_err());
    }
}
