use serde::{Deserialize, Serialize};

use crate::types::{
    Citation, CitationCharLocation, CitationContentBlockLocation, CitationPageLocation,
    CitationSearchResultLocation,
};

/// A delta representing a new citation in a streaming response
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct CitationsDelta {
    /// The citation that was added
    pub citation: Citation,
}

impl CitationsDelta {
    /// Creates a new CitationsDelta with a CitationCharLocation
    #[must_use]
    pub fn new_with_char_location(location: CitationCharLocation) -> Self {
        Self {
            citation: Citation::CharLocation(location),
        }
    }

    /// Creates a new CitationsDelta with a CitationPageLocation
    #[must_use]
    pub fn new_with_page_location(location: CitationPageLocation) -> Self {
        Self {
            citation: Citation::PageLocation(location),
        }
    }

    /// Creates a new CitationsDelta with a CitationContentBlockLocation
    #[must_use]
    pub fn new_with_content_block_location(location: CitationContentBlockLocation) -> Self {
        Self {
            citation: Citation::ContentBlockLocation(location),
        }
    }

    /// Creates a new CitationsDelta with a CitationSearchResultLocation
    #[must_use]
    pub fn new_with_search_result_location(location: CitationSearchResultLocation) -> Self {
        Self {
            citation: Citation::SearchResultLocation(location),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let delta = CitationsDelta::new_with_char_location(char_location);

        let json = serde_json::to_string(&delta).unwrap();
        let expected = r#"{"citation":{"type":"char_location","cited_text":"example text","document_index":0,"document_title":"Document Title","end_char_index":12,"start_char_index":0}}"#;

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

        let delta = CitationsDelta::new_with_page_location(page_location);

        let json = serde_json::to_string(&delta).unwrap();
        let expected = r#"{"citation":{"type":"page_location","cited_text":"example text","document_index":0,"document_title":"Document Title","end_page_number":5,"start_page_number":3}}"#;

        assert_eq!(json, expected);
    }

    #[test]
    fn content_block_location_serialization() {
        let content_block_location = CitationContentBlockLocation {
            cited_text: "example text".to_string(),
            document_index: 0,
            document_title: Some("Document Title".to_string()),
            end_block_index: 3,
            start_block_index: 1,
            file_id: None,
        };

        let delta = CitationsDelta::new_with_content_block_location(content_block_location);

        let json = serde_json::to_string(&delta).unwrap();
        let expected = r#"{"citation":{"type":"content_block_location","cited_text":"example text","document_index":0,"document_title":"Document Title","end_block_index":3,"start_block_index":1}}"#;

        assert_eq!(json, expected);
    }

    #[test]
    fn search_result_location_serialization() {
        let location =
            CitationSearchResultLocation::new("example text", 0, "https://example.com", 1, 3, None);

        let delta = CitationsDelta::new_with_search_result_location(location);

        let json = serde_json::to_string(&delta).unwrap();
        let expected = r#"{"citation":{"type":"search_result_location","cited_text":"example text","search_result_index":0,"source":"https://example.com","start_block_index":1,"end_block_index":3}}"#;

        assert_eq!(json, expected);
    }

    #[test]
    fn deserialization() {
        let char_location_json = r#"{"citation":{"type":"char_location","cited_text":"example text","document_index":0,"document_title":"Document Title","end_char_index":12,"start_char_index":0}}"#;
        let delta: CitationsDelta = serde_json::from_str(char_location_json).unwrap();

        match delta.citation {
            Citation::CharLocation(loc) => {
                assert_eq!(loc.cited_text, "example text");
                assert_eq!(loc.document_index, 0);
                assert_eq!(loc.document_title, Some("Document Title".to_string()));
                assert_eq!(loc.end_char_index, 12);
                assert_eq!(loc.start_char_index, 0);
            }
            _ => panic!("Expected CharLocation"),
        }
    }
}
