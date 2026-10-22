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

        let citation = Citation::CharLocation(char_location);

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

        let citation = Citation::PageLocation(page_location);

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

        let citation = Citation::ContentBlockLocation(content_block_location);

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
        let citation: Citation = serde_json::from_value(json).unwrap();

        match citation {
            Citation::CharLocation(location) => {
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
        let citation = Citation::CharLocation(CitationCharLocation::new(
            "example text",
            0,
            0,
            12,
            Some("Document Title".to_string()),
        ));

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
        let citation = Citation::PageLocation(CitationPageLocation::new(
            "example text",
            0,
            3,
            5,
            Some("Document Title".to_string()),
        ));

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
        let citation = Citation::ContentBlockLocation(CitationContentBlockLocation::new(
            "example text",
            0,
            2,
            4,
            Some("Document Title".to_string()),
        ));

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
        let citation =
            Citation::CharLocation(CitationCharLocation::new("example text", 0, 0, 12, None));

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
        let citation: Citation = serde_json::from_value(json.clone()).unwrap();
        assert!(matches!(citation, Citation::SearchResultLocation(_)));
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
        let citation: Citation = serde_json::from_value(json.clone()).unwrap();
        let Citation::CharLocation(location) = &citation else {
            panic!("expected char_location");
        };
        assert_eq!(location.file_id.as_deref(), Some("file_01"));
        assert_eq!(serde_json::to_value(&citation).unwrap(), json);
    }

    #[test]
    fn unknown_citation_round_trips() {
        let raw = r#"{"type":"future_location","a":{"b":[1,2]}}"#;
        let citation: Citation = serde_json::from_str(raw).unwrap();
        assert!(matches!(citation, Citation::Unknown(_)));
        assert_eq!(serde_json::to_string(&citation).unwrap(), raw);
    }

    #[test]
    fn malformed_known_citation_errors() {
        let json = json!({"type": "char_location"});
        assert!(serde_json::from_value::<Citation>(json).is_err());
    }
}
