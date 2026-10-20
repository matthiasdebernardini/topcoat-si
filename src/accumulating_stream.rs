//! Accumulates streaming events into a complete message while passing events through.

use std::pin::Pin;

use futures::Stream;
use serde_json::Value;

use crate::{
    CacheControlEphemeral, Citation, ContentBlock, ContentBlockDelta, Error, McpToolUseBlock,
    Message, MessageStreamEvent, ServerToolUseBlock, StopReason, TextBlock, TextCitation,
    ThinkingBlock, ToolUseBlock,
};

/// A stream wrapper that accumulates `MessageStreamEvent`s into a complete `Message`.
///
/// This allows streaming tokens to the user while simultaneously building the final message
/// without buffering. When the stream is fully drained, the accumulated message is sent via
/// the oneshot channel returned by `new()`.
pub struct AccumulatingStream {
    inner: Pin<Box<dyn Stream<Item = Result<MessageStreamEvent, Error>> + Send>>,
    message_tx: Option<tokio::sync::oneshot::Sender<Result<Message, Error>>>,
    message: Option<Message>,
    content_blocks: Vec<ContentBlockBuilder>,
    done: bool,
    completed: bool,
}

impl AccumulatingStream {
    /// Wraps a `MessageStreamEvent` stream to accumulate events into a `Message`.
    ///
    /// Returns the stream and a receiver that will contain the accumulated `Message` once the
    /// stream is fully drained.
    pub fn new<S>(stream: S) -> (Self, tokio::sync::oneshot::Receiver<Result<Message, Error>>)
    where
        S: Stream<Item = Result<MessageStreamEvent, Error>> + Send + 'static,
    {
        Self::new_with_message(stream, None)
    }

    /// Wraps a `MessageStreamEvent` stream and seeds accumulation with a fallback message.
    pub fn new_with_message<S>(
        stream: S,
        message: impl Into<Option<Message>>,
    ) -> (Self, tokio::sync::oneshot::Receiver<Result<Message, Error>>)
    where
        S: Stream<Item = Result<MessageStreamEvent, Error>> + Send + 'static,
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let this = Self {
            inner: Box::pin(stream),
            message_tx: Some(tx),
            message: message.into(),
            content_blocks: Vec::new(),
            done: false,
            completed: false,
        };
        (this, rx)
    }

    fn accumulate_event(&mut self, event: &MessageStreamEvent) -> Result<(), Error> {
        match event {
            MessageStreamEvent::MessageStart(start) => {
                self.message = Some(start.message.clone());
            }
            MessageStreamEvent::ContentBlockStart(start) => {
                let idx = start.index;
                if idx > self.content_blocks.len() {
                    return Err(Error::streaming("non-contiguous content block index", None));
                }
                if idx == self.content_blocks.len() {
                    self.content_blocks.push(ContentBlockBuilder::Empty);
                }
                self.content_blocks[idx] =
                    ContentBlockBuilder::from_content_block(start.content_block.clone());
            }
            MessageStreamEvent::ContentBlockDelta(delta_event) => {
                let idx = delta_event.index;
                if idx < self.content_blocks.len() {
                    self.content_blocks[idx].apply_delta(delta_event.delta.clone());
                }
            }
            MessageStreamEvent::ContentBlockStop(_) => {}
            MessageStreamEvent::MessageDelta(delta_event) => {
                if let Some(ref mut msg) = self.message {
                    if delta_event.delta.stop_reason.is_some() {
                        msg.stop_reason = delta_event.delta.stop_reason;
                    }
                    if delta_event.delta.stop_sequence.is_some() {
                        msg.stop_sequence = delta_event.delta.stop_sequence.clone();
                    }
                    if delta_event.delta.stop_details.is_some() {
                        msg.stop_details = delta_event.delta.stop_details.clone();
                    }
                    if delta_event.delta.container.is_some() {
                        msg.container = delta_event.delta.container.clone();
                    }
                    if delta_event.context_management.is_some() {
                        msg.context_management = delta_event.context_management.clone();
                    }
                    delta_event.usage.merge_into(&mut msg.usage);
                }
            }
            MessageStreamEvent::MessageStop(_) => {
                self.completed = true;
            }
            MessageStreamEvent::Ping | MessageStreamEvent::Unknown => {}
        }
        Ok(())
    }

    fn fail(&mut self, error: Error) -> std::task::Poll<Option<Result<MessageStreamEvent, Error>>> {
        self.done = true;
        if let Some(tx) = self.message_tx.take() {
            // The caller may drop the final-message receiver while still reading events.
            let _ = tx.send(Err(error.clone()));
        }
        std::task::Poll::Ready(Some(Err(error)))
    }

    fn finalize(&mut self) -> Result<Message, Error> {
        let mut msg = self
            .message
            .take()
            .ok_or_else(|| Error::streaming("stream ended without a message start event", None))?;
        msg.content = std::mem::take(&mut self.content_blocks)
            .into_iter()
            .filter_map(|builder| builder.build(msg.stop_reason).transpose())
            .collect::<Result<Vec<_>, _>>()?;
        Ok(msg)
    }

    /// Finalizes the currently accumulated message without draining the stream.
    pub fn finalize_partial(&mut self) -> Result<Message, Error> {
        self.done = true;
        self.message_tx.take();
        self.finalize()
    }
}

impl Stream for AccumulatingStream {
    type Item = Result<MessageStreamEvent, Error>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        if self.done {
            return std::task::Poll::Ready(None);
        }
        match self.inner.as_mut().poll_next(cx) {
            std::task::Poll::Ready(Some(Ok(event))) => {
                if let Err(error) = self.accumulate_event(&event) {
                    return self.fail(error);
                }
                std::task::Poll::Ready(Some(Ok(event)))
            }
            std::task::Poll::Ready(Some(Err(error))) => self.fail(error),
            std::task::Poll::Ready(None) => {
                if !self.completed
                    && !self
                        .message
                        .as_ref()
                        .is_some_and(|msg| msg.stop_reason.is_some())
                {
                    let reason = if self.message.is_none() {
                        "stream ended without a message start event"
                    } else {
                        "stream ended before message completion"
                    };
                    return self.fail(Error::streaming(reason, None));
                }
                let result = self.finalize();
                self.done = true;
                if let Some(tx) = self.message_tx.take() {
                    // The receiver is optional; stream items still report any failure.
                    let _ = tx.send(result.clone());
                }
                std::task::Poll::Ready(result.err().map(Err))
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

enum ToolKind {
    Client,
    Server,
    Mcp { server_name: String },
}

enum ContentBlockBuilder {
    Empty,
    Text {
        text: String,
        citations: Option<Vec<TextCitation>>,
        cache_control: Option<CacheControlEphemeral>,
    },
    ToolUse {
        kind: ToolKind,
        id: String,
        name: String,
        input_json: String,
        input_value: Option<Value>,
        saw_delta: bool,
        cache_control: Option<CacheControlEphemeral>,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
    Complete(ContentBlock),
}

impl ContentBlockBuilder {
    fn from_content_block(block: ContentBlock) -> Self {
        match block {
            ContentBlock::Text(text_block) => ContentBlockBuilder::Text {
                text: text_block.text,
                citations: text_block.citations,
                cache_control: text_block.cache_control,
            },
            ContentBlock::ToolUse(tool_use) => ContentBlockBuilder::ToolUse {
                kind: ToolKind::Client,
                id: tool_use.id,
                name: tool_use.name,
                input_json: String::new(),
                input_value: Some(tool_use.input),
                saw_delta: false,
                cache_control: tool_use.cache_control,
            },
            ContentBlock::ServerToolUse(server_tool_use) => ContentBlockBuilder::ToolUse {
                kind: ToolKind::Server,
                id: server_tool_use.id,
                name: server_tool_use.name,
                input_json: String::new(),
                input_value: Some(server_tool_use.input),
                saw_delta: false,
                cache_control: server_tool_use.cache_control,
            },
            ContentBlock::McpToolUse(mcp_tool_use) => ContentBlockBuilder::ToolUse {
                kind: ToolKind::Mcp {
                    server_name: mcp_tool_use.server_name,
                },
                id: mcp_tool_use.id,
                name: mcp_tool_use.name,
                input_json: String::new(),
                input_value: Some(mcp_tool_use.input),
                saw_delta: false,
                cache_control: None,
            },
            ContentBlock::Thinking(thinking) => ContentBlockBuilder::Thinking {
                thinking: thinking.thinking,
                signature: thinking.signature,
            },
            other => ContentBlockBuilder::Complete(other),
        }
    }

    fn apply_delta(&mut self, delta: ContentBlockDelta) {
        match (self, delta) {
            (ContentBlockBuilder::Text { text, .. }, ContentBlockDelta::TextDelta(text_delta)) => {
                text.push_str(&text_delta.text);
            }
            (
                ContentBlockBuilder::Text { citations, .. },
                ContentBlockDelta::CitationsDelta(citations_delta),
            ) => {
                let citation = match citations_delta.citation {
                    Citation::CharLocation(loc) => TextCitation::CharLocation(loc),
                    Citation::PageLocation(loc) => TextCitation::PageLocation(loc),
                    Citation::ContentBlockLocation(loc) => TextCitation::ContentBlockLocation(loc),
                    Citation::WebSearchResultLocation(loc) => {
                        TextCitation::WebSearchResultLocation(loc)
                    }
                    Citation::SearchResultLocation(loc) => TextCitation::SearchResultLocation(loc),
                    Citation::Unknown(value) => TextCitation::Unknown(value),
                };
                citations.get_or_insert_with(Vec::new).push(citation);
            }
            (
                ContentBlockBuilder::ToolUse {
                    input_json,
                    saw_delta,
                    ..
                },
                ContentBlockDelta::InputJsonDelta(json_delta),
            ) => {
                *saw_delta = true;
                input_json.push_str(&json_delta.partial_json);
            }
            (
                ContentBlockBuilder::Thinking { thinking, .. },
                ContentBlockDelta::ThinkingDelta(thinking_delta),
            ) => {
                thinking.push_str(&thinking_delta.thinking);
            }
            (
                ContentBlockBuilder::Thinking { signature, .. },
                ContentBlockDelta::SignatureDelta(sig_delta),
            ) => {
                signature.push_str(&sig_delta.signature);
            }
            (
                ContentBlockBuilder::Complete(ContentBlock::Compaction(block)),
                ContentBlockDelta::CompactionDelta(delta),
            ) => {
                block.content = delta.content;
                block.encrypted_content = delta.encrypted_content;
            }
            _ => {}
        }
    }

    fn build(self, stop_reason: Option<StopReason>) -> Result<Option<ContentBlock>, Error> {
        match self {
            ContentBlockBuilder::Empty => Ok(None),
            ContentBlockBuilder::Text {
                text,
                citations,
                cache_control,
            } => Ok(Some(ContentBlock::Text(TextBlock {
                text,
                citations,
                cache_control,
            }))),
            ContentBlockBuilder::ToolUse {
                kind,
                id,
                name,
                input_json,
                input_value,
                saw_delta,
                cache_control,
            } => {
                let input = if saw_delta {
                    if input_json.trim().is_empty() {
                        Value::Object(serde_json::Map::new())
                    } else {
                        match serde_json::from_str::<Value>(&input_json) {
                            Ok(value) => value,
                            Err(_err) => {
                                if stop_reason == Some(StopReason::MaxTokens) {
                                    return Ok(None);
                                }
                                Value::String(input_json)
                            }
                        }
                    }
                } else if let Some(input) = input_value {
                    input
                } else if input_json.trim().is_empty() {
                    Value::Object(serde_json::Map::new())
                } else {
                    match serde_json::from_str::<Value>(&input_json) {
                        Ok(value) => value,
                        Err(_err) => {
                            if stop_reason == Some(StopReason::MaxTokens) {
                                return Ok(None);
                            }
                            Value::String(input_json)
                        }
                    }
                };
                let block = match kind {
                    ToolKind::Client => ContentBlock::ToolUse(ToolUseBlock {
                        id,
                        name,
                        input,
                        cache_control,
                    }),
                    ToolKind::Server => ContentBlock::ServerToolUse(ServerToolUseBlock {
                        id,
                        name,
                        input,
                        cache_control,
                    }),
                    ToolKind::Mcp { server_name } => ContentBlock::McpToolUse(McpToolUseBlock {
                        id,
                        name,
                        server_name,
                        input,
                    }),
                };
                Ok(Some(block))
            }
            ContentBlockBuilder::Thinking {
                thinking,
                signature,
            } => Ok(Some(ContentBlock::Thinking(ThinkingBlock {
                thinking,
                signature,
            }))),
            ContentBlockBuilder::Complete(block) => Ok(Some(block)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ContentBlockDeltaEvent, ContentBlockStartEvent, ContentBlockStopEvent, InputJsonDelta,
        KnownModel, MessageDelta, MessageDeltaEvent, MessageDeltaUsage, MessageStartEvent, Model,
        OutputTokensDetails, TextDelta, Usage,
    };
    use futures::stream;

    /// Verifies that cache tokens from message_start are preserved through streaming.
    #[tokio::test]
    async fn cache_tokens_from_message_start_preserved() {
        // Build a message_start event with cache tokens in the usage
        let usage_with_cache = Usage::new(100, 0)
            .with_cache_creation_input_tokens(50)
            .with_cache_read_input_tokens(25);

        let start_message = Message::new(
            "msg_test".to_string(),
            Vec::new(),
            Model::Known(KnownModel::ClaudeSonnet55),
            usage_with_cache,
        );
        let start_event = MessageStreamEvent::MessageStart(MessageStartEvent::new(start_message));

        // Build content_block_start for text
        let text_block = ContentBlock::Text(TextBlock::new(String::new()));
        let content_start =
            MessageStreamEvent::ContentBlockStart(ContentBlockStartEvent::new(text_block, 0));

        // Build content_block_delta with text
        let text_delta = TextDelta::new("Hello".to_string());
        let content_delta = MessageStreamEvent::ContentBlockDelta(
            crate::ContentBlockDeltaEvent::new(ContentBlockDelta::TextDelta(text_delta), 0),
        );

        // Build message_delta with final output tokens (no cache tokens - they were in start)
        let delta_usage = MessageDeltaUsage::new(10).with_thinking_tokens(7);
        let message_delta = MessageDelta::new().with_stop_reason(StopReason::EndTurn);
        let delta_event =
            MessageStreamEvent::MessageDelta(MessageDeltaEvent::new(message_delta, delta_usage));

        // Create the stream
        let events = vec![
            Ok(start_event),
            Ok(content_start),
            Ok(content_delta),
            Ok(delta_event),
        ];
        let event_stream = stream::iter(events);

        let (mut acc_stream, rx) = AccumulatingStream::new(event_stream);

        // Drain the stream
        use futures::StreamExt;
        while acc_stream.next().await.is_some() {}

        // Get the accumulated message
        let message = rx
            .await
            .expect("channel closed")
            .expect("accumulation failed");

        // Verify cache tokens were preserved from message_start
        // DEBUG: Print what we got
        println!(
            "cache_creation_input_tokens: {:?}",
            message.usage.cache_creation_input_tokens
        );
        println!(
            "cache_read_input_tokens: {:?}",
            message.usage.cache_read_input_tokens
        );

        assert_eq!(
            message.usage.cache_creation_input_tokens,
            Some(50),
            "cache_creation_input_tokens should be preserved from message_start"
        );
        assert_eq!(
            message.usage.cache_read_input_tokens,
            Some(25),
            "cache_read_input_tokens should be preserved from message_start"
        );
        assert_eq!(
            message.usage.output_tokens, 10,
            "output_tokens should be from message_delta"
        );
        assert_eq!(
            message.usage.output_tokens_details,
            Some(OutputTokensDetails::new(7)),
            "output_tokens_details should be from message_delta"
        );
    }

    /// Verifies that tool use with empty input JSON becomes an empty object, not null.
    #[tokio::test]
    async fn empty_tool_input_becomes_empty_object() {
        let usage = Usage::new(100, 0);
        let start_message = Message::new(
            "msg_test".to_string(),
            Vec::new(),
            Model::Known(KnownModel::ClaudeSonnet55),
            usage,
        );
        let start_event = MessageStreamEvent::MessageStart(MessageStartEvent::new(start_message));

        // Build content_block_start for tool_use with initial empty input
        let tool_use_block =
            ContentBlock::ToolUse(ToolUseBlock::new("tool_123", "get_document", Value::Null));
        let content_start =
            MessageStreamEvent::ContentBlockStart(ContentBlockStartEvent::new(tool_use_block, 0));

        // Build content_block_delta with empty JSON (simulating no input parameters)
        let json_delta = InputJsonDelta::new(String::new());
        let content_delta = MessageStreamEvent::ContentBlockDelta(ContentBlockDeltaEvent::new(
            ContentBlockDelta::InputJsonDelta(json_delta),
            0,
        ));

        // Build content_block_stop
        let content_stop = MessageStreamEvent::ContentBlockStop(ContentBlockStopEvent::new(0));

        // Build message_delta
        let delta_usage = MessageDeltaUsage::new(10);
        let message_delta = MessageDelta::new().with_stop_reason(StopReason::ToolUse);
        let delta_event =
            MessageStreamEvent::MessageDelta(MessageDeltaEvent::new(message_delta, delta_usage));

        let events = vec![
            Ok(start_event),
            Ok(content_start),
            Ok(content_delta),
            Ok(content_stop),
            Ok(delta_event),
        ];
        let event_stream = stream::iter(events);

        let (mut acc_stream, rx) = AccumulatingStream::new(event_stream);

        use futures::StreamExt;
        while acc_stream.next().await.is_some() {}

        let message = rx
            .await
            .expect("channel closed")
            .expect("accumulation failed");

        assert_eq!(message.content.len(), 1, "Should have one content block");
        let tool_use = message.content[0]
            .as_tool_use()
            .expect("Expected ToolUseBlock");

        // Empty input should be an empty object, not null
        assert!(
            tool_use.input.is_object(),
            "Empty tool input should be an object, not null. Got: {:?}",
            tool_use.input
        );
        assert!(
            tool_use
                .input
                .as_object()
                .expect("input should be object")
                .is_empty(),
            "Empty tool input should be an empty object"
        );
        println!("tool_use.input: {:?}", tool_use.input);
    }

    /// Verifies that tool use with no delta events uses initial input_value.
    #[tokio::test]
    async fn tool_input_without_delta_uses_initial_value() {
        let usage = Usage::new(100, 0);
        let start_message = Message::new(
            "msg_test".to_string(),
            Vec::new(),
            Model::Known(KnownModel::ClaudeSonnet55),
            usage,
        );
        let start_event = MessageStreamEvent::MessageStart(MessageStartEvent::new(start_message));

        // Build content_block_start for tool_use with an actual input value
        let input = serde_json::json!({"key": "value"});
        let tool_use_block =
            ContentBlock::ToolUse(ToolUseBlock::new("tool_123", "get_document", input.clone()));
        let content_start =
            MessageStreamEvent::ContentBlockStart(ContentBlockStartEvent::new(tool_use_block, 0));

        // No delta events - the input should come from the initial value

        let content_stop = MessageStreamEvent::ContentBlockStop(ContentBlockStopEvent::new(0));

        let delta_usage = MessageDeltaUsage::new(10);
        let message_delta = MessageDelta::new().with_stop_reason(StopReason::ToolUse);
        let delta_event =
            MessageStreamEvent::MessageDelta(MessageDeltaEvent::new(message_delta, delta_usage));

        let events = vec![
            Ok(start_event),
            Ok(content_start),
            Ok(content_stop),
            Ok(delta_event),
        ];
        let event_stream = stream::iter(events);

        let (mut acc_stream, rx) = AccumulatingStream::new(event_stream);

        use futures::StreamExt;
        while acc_stream.next().await.is_some() {}

        let message = rx
            .await
            .expect("channel closed")
            .expect("accumulation failed");

        let tool_use = message.content[0]
            .as_tool_use()
            .expect("Expected ToolUseBlock");

        assert_eq!(
            tool_use.input, input,
            "Tool input should match initial value"
        );
        println!("tool_use.input: {:?}", tool_use.input);
    }

    /// Verifies that container and diagnostics from message_start and
    /// context_management from message_delta reach the final message.
    #[tokio::test]
    async fn response_fields_are_carried_into_final_message() {
        use crate::{
            AppliedEdit, CacheDiagnostics, CacheMissReason, ContainerInfo,
            ContextManagementResponse,
        };

        let mut start_message = Message::new(
            "msg_test".to_string(),
            Vec::new(),
            Model::Known(KnownModel::ClaudeSonnet55),
            Usage::new(100, 0),
        );
        start_message.container = Some(ContainerInfo {
            id: "container_1".to_string(),
            expires_at: None,
            skills: None,
        });
        start_message.diagnostics = Some(CacheDiagnostics {
            cache_miss_reason: Some(CacheMissReason {
                r#type: CacheMissReason::SYSTEM_CHANGED.to_string(),
                cache_missed_input_tokens: Some(10),
            }),
        });

        let mut message_delta = MessageDelta::new().with_stop_reason(StopReason::EndTurn);
        message_delta.container = Some(ContainerInfo {
            id: "container_2".to_string(),
            expires_at: None,
            skills: None,
        });
        let mut delta_event = MessageDeltaEvent::new(message_delta, MessageDeltaUsage::new(5));
        delta_event.context_management = Some(ContextManagementResponse {
            applied_edits: vec![AppliedEdit {
                r#type: "clear_tool_uses_20250919".to_string(),
                cleared_input_tokens: Some(12000),
                cleared_tool_uses: Some(4),
                cleared_thinking_turns: None,
            }],
        });

        let events = vec![
            Ok(MessageStreamEvent::MessageStart(MessageStartEvent::new(
                start_message,
            ))),
            Ok(MessageStreamEvent::MessageDelta(delta_event)),
            Ok(MessageStreamEvent::MessageStop(
                crate::MessageStopEvent::new(),
            )),
        ];
        let (mut stream, rx) = AccumulatingStream::new(stream::iter(events));
        while futures::StreamExt::next(&mut stream).await.is_some() {}
        let message = rx.await.unwrap().unwrap();

        assert_eq!(message.container.unwrap().id, "container_2");
        assert_eq!(
            message
                .diagnostics
                .unwrap()
                .cache_miss_reason
                .unwrap()
                .r#type,
            "system_changed"
        );
        assert_eq!(
            message.context_management.unwrap().applied_edits[0].cleared_tool_uses,
            Some(4)
        );
    }

    /// Runs one content block through the accumulator and returns the final content.
    async fn accumulate_block(
        start_block: ContentBlock,
        deltas: Vec<ContentBlockDelta>,
    ) -> Vec<ContentBlock> {
        use futures::StreamExt;

        let start_message = Message::new(
            "msg_test".to_string(),
            Vec::new(),
            Model::Known(KnownModel::ClaudeSonnet55),
            Usage::new(1, 0),
        );
        let mut events = vec![
            Ok(MessageStreamEvent::MessageStart(MessageStartEvent::new(
                start_message,
            ))),
            Ok(MessageStreamEvent::ContentBlockStart(
                ContentBlockStartEvent::new(start_block, 0),
            )),
        ];
        events.extend(deltas.into_iter().map(|delta| {
            Ok(MessageStreamEvent::ContentBlockDelta(
                ContentBlockDeltaEvent::new(delta, 0),
            ))
        }));
        events.push(Ok(MessageStreamEvent::ContentBlockStop(
            ContentBlockStopEvent::new(0),
        )));
        events.push(Ok(MessageStreamEvent::MessageDelta(
            MessageDeltaEvent::new(
                MessageDelta::new().with_stop_reason(StopReason::EndTurn),
                MessageDeltaUsage::new(1),
            ),
        )));

        let (mut acc_stream, rx) = AccumulatingStream::new(stream::iter(events));
        while acc_stream.next().await.is_some() {}
        rx.await
            .expect("channel closed")
            .expect("accumulation failed")
            .content
    }

    #[tokio::test]
    async fn complete_compaction_block_passes_through() {
        let block =
            ContentBlock::Compaction(crate::CompactionBlock::new("summary").with_signature("sig"));
        let content = accumulate_block(block.clone(), Vec::new()).await;
        assert_eq!(content, vec![block]);
    }

    #[tokio::test]
    async fn compaction_delta_sets_content() {
        let start = ContentBlock::Compaction(crate::CompactionBlock::default());
        let content = accumulate_block(
            start,
            vec![
                ContentBlockDelta::CompactionDelta(crate::CompactionDelta {
                    content: Some("the summary".to_string()),
                    encrypted_content: None,
                }),
                ContentBlockDelta::Unknown,
            ],
        )
        .await;
        let ContentBlock::Compaction(block) = &content[0] else {
            panic!("expected compaction block");
        };
        assert_eq!(block.content.as_deref(), Some("the summary"));
    }

    #[tokio::test]
    async fn mcp_tool_use_accumulates_input_json_deltas() {
        let start = ContentBlock::McpToolUse(McpToolUseBlock {
            id: "mcptoolu_1".to_string(),
            name: "lookup".to_string(),
            server_name: "docs".to_string(),
            input: serde_json::json!({}),
        });
        let content = accumulate_block(
            start,
            vec![
                ContentBlockDelta::InputJsonDelta(InputJsonDelta::new(r#"{"query":"#.to_string())),
                ContentBlockDelta::InputJsonDelta(InputJsonDelta::new(r#""rust"}"#.to_string())),
            ],
        )
        .await;
        assert_eq!(
            content,
            vec![ContentBlock::McpToolUse(McpToolUseBlock {
                id: "mcptoolu_1".to_string(),
                name: "lookup".to_string(),
                server_name: "docs".to_string(),
                input: serde_json::json!({"query": "rust"}),
            })]
        );
    }

    #[tokio::test]
    async fn compaction_delta_with_null_content_sets_encrypted_content() {
        let start = ContentBlock::Compaction(crate::CompactionBlock::new("stale"));
        let delta: ContentBlockDelta = serde_json::from_value(serde_json::json!({
            "type": "compaction_delta",
            "content": null,
            "encrypted_content": "opaque"
        }))
        .unwrap();
        let content = accumulate_block(start, vec![delta]).await;
        let ContentBlock::Compaction(block) = &content[0] else {
            panic!("expected compaction block");
        };
        assert_eq!(block.content, None);
        assert_eq!(block.encrypted_content.as_deref(), Some("opaque"));
    }

    #[tokio::test]
    async fn unknown_block_type_passes_through_intact() {
        let raw = serde_json::json!({"type": "some_future_block", "foo": {"bar": [1, 2]}});
        let block: ContentBlock = serde_json::from_value(raw.clone()).unwrap();
        let content = accumulate_block(
            block,
            vec![ContentBlockDelta::TextDelta(TextDelta::new(
                "ignored".to_string(),
            ))],
        )
        .await;
        assert_eq!(content.len(), 1);
        assert_eq!(content[0].as_unknown(), Some(&raw));
    }
}
