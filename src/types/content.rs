use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::types::open_union;
use crate::types::{DocumentBlock, ImageBlock, SearchResultBlock, TextBlock, ToolReference};

/// A content item inside a tool result: text, image, document, search result, tool
/// reference, or an unrecognized item kept verbatim.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum Content {
    /// A text block content.
    #[serde(rename = "text")]
    Text(TextBlock),

    /// An image block content.
    #[serde(rename = "image")]
    Image(ImageBlock),

    /// A document block content.
    #[serde(rename = "document")]
    Document(DocumentBlock),

    /// A search result block content.
    #[serde(rename = "search_result")]
    SearchResult(SearchResultBlock),

    /// A reference to a tool discovered by tool search.
    #[serde(rename = "tool_reference")]
    ToolReference(ToolReference),

    /// An item of a type this crate does not know, kept verbatim.
    #[serde(untagged)]
    Unknown(Value),
}

impl<'de> Deserialize<'de> for Content {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        match open_union::type_tag::<D::Error>(&value)?.as_str() {
            "text" => open_union::from_value(value).map(Content::Text),
            "image" => open_union::from_value(value).map(Content::Image),
            "document" => open_union::from_value(value).map(Content::Document),
            "search_result" => open_union::from_value(value).map(Content::SearchResult),
            "tool_reference" => open_union::from_value(value).map(Content::ToolReference),
            tag => {
                open_union::warn_unknown("Content", tag);
                Ok(Content::Unknown(value))
            }
        }
    }
}

impl From<TextBlock> for Content {
    fn from(text_block: TextBlock) -> Self {
        Content::Text(text_block)
    }
}

impl From<ImageBlock> for Content {
    fn from(image_block: ImageBlock) -> Self {
        Content::Image(image_block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Base64ImageSource, ImageMediaType, ImageSource};
    use serde_json::{json, to_value};

    #[test]
    fn content_text_serialization() {
        let text_block = TextBlock::new("Sample text content");
        let content = Content::Text(text_block);

        let json = to_value(&content).unwrap();
        assert_eq!(
            json,
            json!({
                "text": "Sample text content",
                "type": "text"
            })
        );
    }

    #[test]
    fn content_image_serialization() {
        let base64_source = Base64ImageSource::new(
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8/5+hHgAHggJ/PchI7wAAAABJRU5ErkJggg==",
            ImageMediaType::Png,
        );
        let image_block = ImageBlock::new(ImageSource::Base64(base64_source));
        let content = Content::Image(image_block);

        let json = to_value(&content).unwrap();
        assert_eq!(
            json,
            json!({
                "source": {
                    "data": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8/5+hHgAHggJ/PchI7wAAAABJRU5ErkJggg==",
                    "media_type": "image/png",
                    "type": "base64"
                },
                "type": "image"
            })
        );
    }

    #[test]
    fn content_deserialization() {
        // Test text deserialization
        let json = json!({
            "text": "Sample text content",
            "type": "text"
        });

        let content: Content = serde_json::from_value(json).unwrap();
        match content {
            Content::Text(text_block) => {
                assert_eq!(text_block.text, "Sample text content");
            }
            _ => panic!("Expected Text variant"),
        }

        // Test image deserialization
        let json = json!({
            "source": {
                "data": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8/5+hHgAHggJ/PchI7wAAAABJRU5ErkJggg==",
                "media_type": "image/png",
                "type": "base64"
            },
            "type": "image"
        });

        let content: Content = serde_json::from_value(json).unwrap();
        match content {
            Content::Image(_) => {
                // Image deserialization successful
            }
            _ => panic!("Expected Image variant"),
        }
    }

    #[test]
    fn from_text_block() {
        let text_block = TextBlock::new("Test content");
        let content = Content::from(text_block.clone());

        match content {
            Content::Text(block) => {
                assert_eq!(block.text, "Test content");
            }
            _ => panic!("Expected Text variant"),
        }
    }

    #[test]
    fn from_image_block() {
        let base64_source = Base64ImageSource::new("data", ImageMediaType::Png);
        let image_block = ImageBlock::new(ImageSource::Base64(base64_source));
        let content = Content::from(image_block);

        match content {
            Content::Image(_) => {
                // Conversion successful
            }
            _ => panic!("Expected Image variant"),
        }
    }

    #[test]
    fn open_dispatch() {
        let raw = r#"{"type":"future_item","x":[1]}"#;
        let content: Content = serde_json::from_str(raw).unwrap();
        assert!(matches!(content, Content::Unknown(_)));
        assert_eq!(serde_json::to_string(&content).unwrap(), raw);

        let reference: Content =
            serde_json::from_value(json!({"type": "tool_reference", "tool_name": "x"})).unwrap();
        assert_eq!(
            to_value(&reference).unwrap(),
            json!({"type": "tool_reference", "tool_name": "x"})
        );

        let document: Content = serde_json::from_value(
            json!({"type": "document", "source": {"type": "file", "file_id": "f"}}),
        )
        .unwrap();
        assert!(matches!(document, Content::Document(_)));

        let search: Content = serde_json::from_value(json!({
            "type": "search_result", "source": "s", "title": "t",
            "content": [{"type": "text", "text": "hi"}]
        }))
        .unwrap();
        assert!(matches!(search, Content::SearchResult(_)));

        assert!(serde_json::from_value::<Content>(json!({"type": "text"})).is_err());
    }
}
