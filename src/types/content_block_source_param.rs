use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::str::FromStr;

use crate::types::Content;

/// Parameter for a content block source.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct ContentBlockSourceParam {
    /// The content of the source, which can be either a string or an array of content items.
    pub content: ContentBlockSourceContent,
}

/// The content of a content block source, which can be either a string or an array of content items.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ContentBlockSourceContent {
    /// A simple string content.
    String(String),

    /// An array of content items.
    Array(Vec<Content>),
}

impl ContentBlockSourceParam {
    /// Create a new `ContentBlockSourceParam` with a string content.
    #[must_use]
    pub fn new_with_string(content: impl Into<String>) -> Self {
        Self {
            content: ContentBlockSourceContent::String(content.into()),
        }
    }

    /// Create a new `ContentBlockSourceParam` with an array of content items.
    #[must_use]
    pub fn new_with_array(content: Vec<Content>) -> Self {
        Self {
            content: ContentBlockSourceContent::Array(content),
        }
    }

    /// Create a new `ContentBlockSourceParam` with a single content item.
    #[must_use]
    pub fn new_with_item(content: Content) -> Self {
        Self::new_with_array(vec![content])
    }
}

impl FromStr for ContentBlockSourceParam {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::new_with_string(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ImageBlock, TextBlock, UrlImageSource};
    use serde_json::{json, to_value};

    #[test]
    fn content_block_source_param_with_string() {
        let source = ContentBlockSourceParam::new_with_string("Sample content");
        let json = to_value(&source).unwrap();

        assert_eq!(
            json,
            json!({
                "content": "Sample content"
            })
        );
    }

    #[test]
    fn content_block_source_param_with_array() {
        let text_param = TextBlock::new("Sample text content");
        let url_source = UrlImageSource::new("https://example.com/image.jpg");
        let image_param = ImageBlock::new_with_url(url_source);

        let content = vec![Content::Text(text_param), Content::Image(image_param)];

        let source = ContentBlockSourceParam::new_with_array(content);
        let json = to_value(&source).unwrap();

        assert_eq!(
            json,
            json!({
                "content": [
                    {
                        "text": "Sample text content",
                        "type": "text"
                    },
                    {
                        "source": {
                            "url": "https://example.com/image.jpg",
                            "type": "url"
                        },
                        "type": "image"
                    }
                ]
            })
        );
    }

    #[test]
    fn content_block_source_param_with_item() {
        let text_param = TextBlock::new("Sample text content");
        let source = ContentBlockSourceParam::new_with_item(Content::Text(text_param));

        let json = to_value(&source).unwrap();

        assert_eq!(
            json,
            json!({
                "content": [
                    {
                        "text": "Sample text content",
                        "type": "text"
                    }
                ]
            })
        );
    }

    #[test]
    fn new_with_string_from_literal() {
        let source = ContentBlockSourceParam::new_with_string("Sample content");

        match &source.content {
            ContentBlockSourceContent::String(content) => {
                assert_eq!(content, "Sample content");
            }
            _ => panic!("Expected String variant"),
        }
    }

    #[test]
    fn from_str() {
        let source = "Sample content".parse::<ContentBlockSourceParam>().unwrap();

        match &source.content {
            ContentBlockSourceContent::String(content) => {
                assert_eq!(content, "Sample content");
            }
            _ => panic!("Expected String variant"),
        }
    }
}
