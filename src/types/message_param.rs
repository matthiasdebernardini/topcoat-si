use serde::{Deserialize, Deserializer, Serialize};

use crate::types::open_union;
use crate::types::{
    Content, ContentBlock, Message, SearchResultBlock, TextBlock, ToolResultBlockContent,
};

/// The content of a message, which can be either a string or an array of content blocks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum MessageParamContent {
    /// A simple string content.
    String(String),

    /// An array of content blocks.
    Array(Vec<ContentBlock>),
}

/// Parameters for a message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct MessageParam {
    /// The content of the message.
    pub content: MessageParamContent,

    /// The role of the message.
    pub role: MessageRole,
}

/// Rejects the message shapes the API refuses with a 400, before the request leaves.
///
/// A message whose role is [`MessageRole::Unknown`] has no API value to send. A top-level
/// `document` block does not belong in an `assistant` message. A `text` block with
/// citations does not belong in a `user` message, directly or inside a `search_result`
/// block (top level or in a `tool_result`).
pub(crate) fn check_turns(messages: &[MessageParam]) -> crate::Result<()> {
    for (index, message) in messages.iter().enumerate() {
        if message.role == MessageRole::Unknown {
            return Err(open_union::unknown_in_request(format!(
                "messages[{index}].role"
            )));
        }
        let MessageParamContent::Array(blocks) = &message.content else {
            continue;
        };
        for (j, block) in blocks.iter().enumerate() {
            let path = format!("messages[{index}].content[{j}]");
            match (message.role, block) {
                (MessageRole::Assistant, ContentBlock::Document(_)) => {
                    return Err(crate::Error::validation(
                        format!("{path} is a document block; documents belong in user messages"),
                        Some(path),
                    ));
                }
                (MessageRole::User, ContentBlock::Text(text)) => {
                    check_no_citations(text, &format!("{path}.citations"))?;
                }
                (MessageRole::User, ContentBlock::SearchResult(result)) => {
                    check_search_result(result, &path)?;
                }
                (MessageRole::User, ContentBlock::ToolResult(tool_result)) => {
                    if let Some(ToolResultBlockContent::Array(items)) = &tool_result.content {
                        for (k, item) in items.iter().enumerate() {
                            if let Content::SearchResult(result) = item {
                                check_search_result(result, &format!("{path}.content[{k}]"))?;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn check_search_result(result: &SearchResultBlock, path: &str) -> crate::Result<()> {
    for (k, text) in result.content.iter().enumerate() {
        check_no_citations(text, &format!("{path}.content[{k}].citations"))?;
    }
    Ok(())
}

fn check_no_citations(text: &TextBlock, path: &str) -> crate::Result<()> {
    if text.has_citations() {
        return Err(crate::Error::validation(
            format!("{path} has citations; text blocks in user messages cannot carry citations"),
            Some(path.to_string()),
        ));
    }
    Ok(())
}

/// Role type for a message parameter.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum MessageRole {
    /// User role.
    User,

    /// Assistant role.
    Assistant,

    /// System role.
    ///
    /// System messages provide instructions or context that steer the
    /// assistant's behavior. Unlike the top-level system prompt, a system
    /// message can be interleaved at any point in the conversation history.
    System,

    /// A role this crate does not know.
    ///
    /// Never send it; it only models values this crate does not know.
    Unknown,
}

impl<'de> Deserialize<'de> for MessageRole {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        open_union::deserialize_str(
            deserializer,
            "MessageRole",
            |value| match value {
                "user" => Some(Self::User),
                "assistant" => Some(Self::Assistant),
                "system" => Some(Self::System),
                _ => None,
            },
            Self::Unknown,
        )
    }
}

impl MessageParam {
    /// Create a new `MessageParam` with the given content and role.
    #[must_use]
    pub fn new(content: MessageParamContent, role: MessageRole) -> Self {
        Self { content, role }
    }

    /// Create a new `MessageParam` with an array of content blocks.
    #[must_use]
    pub fn new_with_blocks(blocks: Vec<ContentBlock>, role: MessageRole) -> Self {
        Self::new(MessageParamContent::Array(blocks), role)
    }

    /// Create a new user `MessageParam` with a string content.
    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self::new(
            MessageParamContent::String(content.into()),
            MessageRole::User,
        )
    }

    /// Create a new assistant `MessageParam` with a string content.
    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::new(
            MessageParamContent::String(content.into()),
            MessageRole::Assistant,
        )
    }

    /// Echo a reply back into the conversation as an assistant message.
    #[must_use]
    pub fn from_message(message: Message) -> Self {
        Self::new_with_blocks(message.content, MessageRole::Assistant)
    }
}

impl From<&str> for MessageParam {
    fn from(content: &str) -> Self {
        Self::user(content)
    }
}

impl From<String> for MessageParam {
    fn from(content: String) -> Self {
        Self::user(content)
    }
}

impl From<Message> for MessageParam {
    fn from(message: Message) -> Self {
        Self::from_message(message)
    }
}

impl<T: AsRef<str>> From<T> for MessageParamContent {
    fn from(content: T) -> Self {
        MessageParamContent::String(content.as_ref().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        ImageBlock, ImageSource, KnownModel, Message, Model, TextBlock, ToolResultBlock,
        UrlImageSource, Usage, WebFetchToolResultBlock, WebFetchToolResultContent,
    };
    use serde_json::{json, to_value};

    #[test]
    fn message_param_with_string() {
        let message = MessageParam::user("Hello, Claude!");
        let json = to_value(&message).unwrap();

        assert_eq!(
            json,
            json!({
                "content": "Hello, Claude!",
                "role": "user"
            })
        );
    }

    #[test]
    fn message_param_from_str() {
        let message: MessageParam = "Hello, Claude!".into();
        assert_eq!(message.role, MessageRole::User);

        let message = MessageParam::from("Hello from string");
        assert_eq!(message.role, MessageRole::User);
    }

    #[test]
    fn message_param_ergonomic_constructors() {
        // Test the improved user/assistant methods that accept &str
        let user_msg = MessageParam::user("Hello");
        let assistant_msg = MessageParam::assistant("Hi there");

        assert_eq!(user_msg.role, MessageRole::User);
        assert_eq!(assistant_msg.role, MessageRole::Assistant);
    }

    #[test]
    fn message_param_with_blocks() {
        let text_block = TextBlock::new("Hello, Claude!");
        let blocks = vec![ContentBlock::Text(text_block)];

        let message = MessageParam::new_with_blocks(blocks, MessageRole::User);
        let json = to_value(&message).unwrap();

        assert_eq!(
            json,
            json!({
                "content": [
                    {
                        "text": "Hello, Claude!",
                        "type": "text"
                    }
                ],
                "role": "user"
            })
        );
    }

    #[test]
    fn message_param_with_mixed_blocks() {
        let text_block = TextBlock::new("Check out this image:");

        let image_source = UrlImageSource::new("https://example.com/image.jpg");
        let image_block = ImageBlock::new(ImageSource::Url(image_source));

        let blocks = vec![
            ContentBlock::Text(text_block),
            ContentBlock::Image(image_block),
        ];

        let message = MessageParam::new_with_blocks(blocks, MessageRole::User);
        let json = to_value(&message).unwrap();

        assert_eq!(
            json,
            json!({
                "content": [
                    {
                        "text": "Check out this image:",
                        "type": "text"
                    },
                    {
                        "source": {
                            "url": "https://example.com/image.jpg",
                            "type": "url"
                        },
                        "type": "image"
                    }
                ],
                "role": "user"
            })
        );
    }

    #[test]
    fn message_param_content_from_as_ref_str() {
        // Test From<&str>
        let content: MessageParamContent = "Hello".into();
        match content {
            MessageParamContent::String(s) => assert_eq!(s, "Hello"),
            _ => panic!("Expected String variant"),
        }

        // Test From<String>
        let content: MessageParamContent = "World".to_string().into();
        match content {
            MessageParamContent::String(s) => assert_eq!(s, "World"),
            _ => panic!("Expected String variant"),
        }

        // Test From<&String>
        let s = "Test".to_string();
        let content: MessageParamContent = (&s).into();
        match content {
            MessageParamContent::String(s) => assert_eq!(s, "Test"),
            _ => panic!("Expected String variant"),
        }
    }

    #[test]
    fn message_param_deserialization() {
        let json = json!({
            "content": "Hello, Claude!",
            "role": "user"
        });

        let message: MessageParam = serde_json::from_value(json).unwrap();
        match message.content {
            MessageParamContent::String(s) => assert_eq!(s, "Hello, Claude!"),
            _ => panic!("Expected String variant"),
        }
        assert_eq!(message.role, MessageRole::User);

        let json = json!({
            "content": [
                {
                    "text": "Hello, Claude!",
                    "type": "text"
                }
            ],
            "role": "assistant"
        });

        let message: MessageParam = serde_json::from_value(json).unwrap();
        match message.content {
            MessageParamContent::Array(blocks) => {
                assert_eq!(blocks.len(), 1);
                match &blocks[0] {
                    ContentBlock::Text(text) => assert_eq!(text.text, "Hello, Claude!"),
                    _ => panic!("Expected Text variant"),
                }
            }
            _ => panic!("Expected Array variant"),
        }
        assert_eq!(message.role, MessageRole::Assistant);
    }

    #[test]
    fn message_param_from_message() {
        let text_block = TextBlock::new("Hello, I'm Claude.");
        let content = vec![ContentBlock::Text(text_block)];
        let model = Model::Known(KnownModel::ClaudeSonnet55);
        let usage = Usage::new(50, 100);

        let message = Message::new("msg_012345", content, model, usage);
        let param: MessageParam = message.into();

        assert_eq!(param.role, MessageRole::Assistant);
        match param.content {
            MessageParamContent::Array(blocks) => {
                assert_eq!(blocks.len(), 1);
                match &blocks[0] {
                    ContentBlock::Text(text) => assert_eq!(text.text, "Hello, I'm Claude."),
                    _ => panic!("Expected Text variant"),
                }
            }
            _ => panic!("Expected Array variant"),
        }
    }

    #[test]
    fn message_param_deserializes_tool_result_transcript_blocks() {
        let transcript = json!([
            {
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_123",
                        "content": "ok"
                    },
                    {
                        "type": "web_fetch_tool_result",
                        "tool_use_id": "toolu_456",
                        "content": {
                            "type": "web_fetch_tool_result_error",
                            "error_code": "url_not_accessible"
                        }
                    }
                ]
            }
        ]);

        let messages: Vec<MessageParam> = serde_json::from_value(transcript).unwrap();
        assert_eq!(messages.len(), 1);

        let MessageParamContent::Array(blocks) = &messages[0].content else {
            panic!("Expected array content");
        };
        assert!(matches!(&blocks[0], ContentBlock::ToolResult(_)));
        assert!(matches!(&blocks[1], ContentBlock::WebFetchToolResult(_)));
    }

    #[test]
    fn message_param_serializes_tool_result_transcript_blocks_without_duplicate_type() {
        let tool_result = ToolResultBlock::new("toolu_123").with_string_content("ok");
        let web_fetch_tool_result = WebFetchToolResultBlock {
            tool_use_id: "toolu_456".to_string(),
            content: WebFetchToolResultContent::Error {
                error_code: "url_not_accessible".to_string(),
            },
            cache_control: None,
        };

        let message = MessageParam::new_with_blocks(
            vec![
                ContentBlock::ToolResult(tool_result),
                ContentBlock::WebFetchToolResult(web_fetch_tool_result),
            ],
            MessageRole::User,
        );

        let json = to_value(&message).unwrap();
        assert_eq!(
            json,
            json!({
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_123",
                        "content": "ok"
                    },
                    {
                        "type": "web_fetch_tool_result",
                        "tool_use_id": "toolu_456",
                        "content": {
                            "type": "web_fetch_tool_result_error",
                            "error_code": "url_not_accessible"
                        }
                    }
                ]
            })
        );
    }

    mod check_turns {
        use super::*;
        use crate::ErrorKind;
        use crate::types::{
            Citation, CitationCharLocation, DocumentBlock, PlainTextSource,
            WebFetchToolResultBlock, WebFetchToolResultContent,
        };

        fn cited() -> TextBlock {
            TextBlock::new_with_citations(
                "claim",
                vec![Citation::CharLocation(CitationCharLocation::new(
                    "quote", 0, 0, 5, None,
                ))],
            )
        }

        fn document() -> DocumentBlock {
            DocumentBlock::new(PlainTextSource::new("body").into())
        }

        fn turn(role: MessageRole, block: ContentBlock) -> Vec<MessageParam> {
            vec![MessageParam::new_with_blocks(vec![block], role)]
        }

        fn field(messages: &[MessageParam]) -> String {
            match check_turns(messages).unwrap_err().kind() {
                ErrorKind::Validation { param, .. } => param.clone().unwrap(),
                other => panic!("expected a validation error, got {other:?}"),
            }
        }

        #[test]
        fn rejects_assistant_top_level_document() {
            let messages = turn(MessageRole::Assistant, ContentBlock::Document(document()));
            assert_eq!(field(&messages), "messages[0].content[0]");
        }

        #[test]
        fn accepts_assistant_document_nested_in_web_fetch_result() {
            let fetched = WebFetchToolResultBlock {
                tool_use_id: "srvtoolu_1".to_string(),
                content: WebFetchToolResultContent::Result {
                    url: "https://example.com".to_string(),
                    retrieved_at: None,
                    content: document(),
                },
                cache_control: None,
            };
            let messages = turn(
                MessageRole::Assistant,
                ContentBlock::WebFetchToolResult(fetched),
            );
            assert!(check_turns(&messages).is_ok());
        }

        #[test]
        fn rejects_user_text_with_citations() {
            let messages = turn(MessageRole::User, ContentBlock::Text(cited()));
            assert_eq!(field(&messages), "messages[0].content[0].citations");
        }

        #[test]
        fn rejects_user_search_result_text_with_citations() {
            let result = SearchResultBlock::new("src", "title", vec![cited()]);
            let messages = turn(MessageRole::User, ContentBlock::SearchResult(result));
            assert_eq!(
                field(&messages),
                "messages[0].content[0].content[0].citations"
            );
        }

        #[test]
        fn rejects_user_tool_result_search_result_with_citations() {
            let result = SearchResultBlock::new("src", "title", vec![cited()]);
            let mut tool_result = ToolResultBlock::new("toolu_1");
            tool_result.content = Some(ToolResultBlockContent::Array(vec![Content::SearchResult(
                result,
            )]));
            let messages = turn(MessageRole::User, ContentBlock::ToolResult(tool_result));
            assert_eq!(
                field(&messages),
                "messages[0].content[0].content[0].content[0].citations"
            );
        }

        #[test]
        fn accepts_assistant_text_with_citations() {
            let messages = turn(MessageRole::Assistant, ContentBlock::Text(cited()));
            assert!(check_turns(&messages).is_ok());
        }

        #[test]
        fn accepts_empty_citations() {
            let mut text = TextBlock::new("plain");
            text.citations = Some(vec![]);
            let messages = turn(MessageRole::User, ContentBlock::Text(text));
            assert!(check_turns(&messages).is_ok());
        }
    }
}
