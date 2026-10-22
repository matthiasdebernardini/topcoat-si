use serde::{Deserialize, Serialize};

use crate::types::TextBlock;
use crate::types::wire_tag::TextTag;

/// A wrapper around TextBlock for system prompts that includes a type field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SystemTextBlock {
    /// Block type, always `"text"`.
    r#type: TextTag,

    /// The text content.
    #[serde(flatten)]
    pub block: TextBlock,
}

/// Represents either a string or an array of TextBlockParam for system prompts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SystemPrompt {
    /// A simple string system prompt.
    String(String),

    /// An array of text block parameters.
    Blocks(Vec<SystemTextBlock>),
}

impl From<String> for SystemPrompt {
    fn from(content: String) -> Self {
        Self::String(content)
    }
}

impl From<&str> for SystemPrompt {
    fn from(content: &str) -> Self {
        Self::String(content.to_string())
    }
}

impl From<Vec<TextBlock>> for SystemPrompt {
    fn from(blocks: Vec<TextBlock>) -> Self {
        let system_blocks = blocks
            .into_iter()
            .map(|block| SystemTextBlock {
                r#type: TextTag,
                block,
            })
            .collect();
        Self::Blocks(system_blocks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

    #[test]
    fn system_prompt_string() {
        let prompt = SystemPrompt::String("You are a helpful assistant.".to_string());
        let json = to_value(&prompt).unwrap();
        assert_eq!(json, json!("You are a helpful assistant."));
    }

    #[test]
    fn system_prompt_blocks() {
        let blocks = vec![TextBlock::new("You are a helpful assistant.")];
        let prompt = SystemPrompt::from(blocks);
        let json = to_value(&prompt).unwrap();
        assert_eq!(
            json,
            json!([{
                "text": "You are a helpful assistant.",
                "type": "text"
            }])
        );
    }

    #[test]
    fn from_string() {
        let prompt = SystemPrompt::from("Hello");
        assert_eq!(prompt, SystemPrompt::String("Hello".to_string()));

        let prompt: SystemPrompt = "Hello".into();
        assert_eq!(prompt, SystemPrompt::String("Hello".to_string()));

        let prompt: SystemPrompt = "Hello".to_string().into();
        assert_eq!(prompt, SystemPrompt::String("Hello".to_string()));
    }

    #[test]
    fn from_blocks() {
        let blocks = vec![TextBlock::new("Hello")];
        let expected_blocks = vec![SystemTextBlock {
            r#type: TextTag,
            block: TextBlock::new("Hello"),
        }];
        let prompt: SystemPrompt = blocks.into();
        assert_eq!(prompt, SystemPrompt::Blocks(expected_blocks));
    }
}
