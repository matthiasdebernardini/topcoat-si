//! Accumulates streaming events into a complete message while passing events through.

use std::pin::Pin;
use std::task::{Context, Poll};

use futures::Stream;
use serde_json::Value;
use tokio::sync::oneshot;

use crate::error::describe_json_error;
use crate::{
    CacheControlEphemeral, Citation, ContentBlock, ContentBlockDelta, Error, ErrorKind,
    McpToolUseBlock, Message, MessageStreamEvent, ServerToolUseBlock, StopReason, TextBlock,
    ThinkingBlock, ToolUseBlock,
};

/// A stream wrapper that accumulates `MessageStreamEvent`s into a complete `Message`.
///
/// This allows streaming tokens to the user while simultaneously building the final message
/// without buffering. When the stream is fully drained after `message_stop`, the accumulated
/// message is sent via the oneshot channel returned by `new()`. A stream that fails sends the
/// error instead, and one that ends before `message_stop` closes the channel without a message.
#[must_use = "streams do nothing unless polled"]
pub struct AccumulatingStream {
    inner: Pin<Box<dyn Stream<Item = Result<MessageStreamEvent, Error>> + Send>>,
    message_tx: Option<oneshot::Sender<Result<Message, Error>>>,
    message: Option<Message>,
    content_blocks: Vec<ContentBlockBuilder>,
    done: bool,
    completed: bool,
    /// Events other than pings yielded so far, recorded on the error that ends the stream.
    delivered: u64,
}

impl AccumulatingStream {
    /// Wraps a `MessageStreamEvent` stream to accumulate events into a `Message`.
    ///
    /// Returns the stream and a receiver that will contain the accumulated `Message` once the
    /// stream is fully drained after `message_stop`.
    pub fn new<S>(stream: S) -> (Self, oneshot::Receiver<Result<Message, Error>>)
    where
        S: Stream<Item = Result<MessageStreamEvent, Error>> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let this = Self {
            inner: Box::pin(stream),
            message_tx: Some(tx),
            message: None,
            content_blocks: Vec::new(),
            done: false,
            completed: false,
            delivered: 0,
        };
        (this, rx)
    }

    fn accumulate_event(&mut self, event: &MessageStreamEvent) -> Result<(), Error> {
        match event {
            MessageStreamEvent::MessageStart(start) => {
                self.message = Some(start.message.clone());
            }
            MessageStreamEvent::ContentBlockStart(start) => {
                let index = start.index;
                if index > self.content_blocks.len() {
                    return Err(Error::new(
                        ErrorKind::Decode,
                        "non-contiguous content block index",
                    ));
                }
                if index == self.content_blocks.len() {
                    self.content_blocks.push(ContentBlockBuilder::Empty);
                }
                self.content_blocks[index] =
                    ContentBlockBuilder::from_content_block(start.content_block.clone());
            }
            MessageStreamEvent::ContentBlockDelta(delta_event) => {
                let index = delta_event.index;
                let Some(block) = self.content_blocks.get_mut(index) else {
                    return Err(Error::new(
                        ErrorKind::Decode,
                        format!(
                            "content block delta for index {index}, but only {} blocks have started",
                            self.content_blocks.len()
                        ),
                    ));
                };
                block.apply_delta(delta_event.delta.clone());
            }
            MessageStreamEvent::ContentBlockStop(_) => {}
            MessageStreamEvent::MessageDelta(delta_event) => {
                if let Some(ref mut message) = self.message {
                    if delta_event.delta.stop_reason.is_some() {
                        message.stop_reason = delta_event.delta.stop_reason;
                    }
                    if delta_event.delta.stop_sequence.is_some() {
                        message.stop_sequence = delta_event.delta.stop_sequence.clone();
                    }
                    if delta_event.delta.stop_details.is_some() {
                        message.stop_details = delta_event.delta.stop_details.clone();
                    }
                    if delta_event.delta.container.is_some() {
                        message.container = delta_event.delta.container.clone();
                    }
                    if delta_event.context_management.is_some() {
                        message.context_management = delta_event.context_management.clone();
                    }
                    delta_event.usage.merge_into(&mut message.usage);
                }
            }
            MessageStreamEvent::MessageStop(_) => {
                self.completed = true;
            }
            MessageStreamEvent::Ping | MessageStreamEvent::Unknown => {}
        }
        Ok(())
    }

    /// Ends the stream with `error`. An error from the inner stream keeps the count it carries,
    /// such as the one [`crate::process_message_stream_sse`] sets.
    fn fail(&mut self, error: Error) -> Poll<Option<Result<MessageStreamEvent, Error>>> {
        let error = match error.events_delivered() {
            Some(_) => error,
            None => error.with_events_delivered(self.delivered),
        };
        self.done = true;
        if let Some(tx) = self.message_tx.take() {
            // The caller may drop the final-message receiver while still reading events.
            let _ = tx.send(Err(error.clone()));
        }
        Poll::Ready(Some(Err(error)))
    }

    fn finalize(&mut self) -> Result<Message, Error> {
        let mut message = self.message.take().ok_or_else(|| {
            // After `message_stop`, a missing `message_start` is malformed server output. On a
            // stream still running (`finalize_partial`), nothing has gone wrong yet.
            let kind = if self.completed {
                ErrorKind::Decode
            } else {
                ErrorKind::Interrupted
            };
            Error::new(kind, "stream ended without a message start event")
        })?;
        message.content = std::mem::take(&mut self.content_blocks)
            .into_iter()
            .filter_map(|builder| builder.build(message.stop_reason).transpose())
            .collect::<Result<Vec<_>, _>>()?;
        Ok(message)
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

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.done {
            return Poll::Ready(None);
        }
        match self.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(event))) => {
                if let Err(error) = self.accumulate_event(&event) {
                    return self.fail(error);
                }
                if !matches!(event, MessageStreamEvent::Ping) {
                    self.delivered += 1;
                }
                Poll::Ready(Some(Ok(event)))
            }
            Poll::Ready(Some(Err(error))) => self.fail(error),
            Poll::Ready(None) => {
                self.done = true;
                if !self.completed {
                    // `Anthropic::stream` already ended with an error; another stream that ends
                    // early closes the channel without a message.
                    self.message_tx.take();
                    return Poll::Ready(None);
                }
                match self.finalize() {
                    Ok(message) => {
                        self.done = true;
                        if let Some(tx) = self.message_tx.take() {
                            // The receiver is optional; stream items still report any failure.
                            let _ = tx.send(Ok(message));
                        }
                        Poll::Ready(None)
                    }
                    Err(error) => self.fail(error),
                }
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Parses the JSON a tool call's `input_json_delta` events added up to. Empty input is an empty
/// object. The error gives where parsing failed and never the text.
fn parse_tool_input(name: &str, input_json: &str) -> Result<Value, Error> {
    if input_json.trim().is_empty() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    serde_json::from_str(input_json).map_err(|error| {
        Error::new(
            ErrorKind::Decode,
            format!(
                "the input of the {name} tool call ({} bytes) is not valid JSON: {}",
                input_json.len(),
                describe_json_error(&error)
            ),
        )
    })
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
        citations: Option<Vec<Citation>>,
        cache_control: Option<CacheControlEphemeral>,
    },
    ToolUse {
        kind: ToolKind,
        id: String,
        name: String,
        input_json: String,
        input_value: Value,
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
                input_value: tool_use.input,
                saw_delta: false,
                cache_control: tool_use.cache_control,
            },
            ContentBlock::ServerToolUse(server_tool_use) => ContentBlockBuilder::ToolUse {
                kind: ToolKind::Server,
                id: server_tool_use.id,
                name: server_tool_use.name,
                input_json: String::new(),
                input_value: server_tool_use.input,
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
                input_value: mcp_tool_use.input,
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
                citations
                    .get_or_insert_with(Vec::new)
                    .push(citations_delta.citation);
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
                ContentBlockDelta::SignatureDelta(signature_delta),
            ) => {
                signature.push_str(&signature_delta.signature);
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
                    match parse_tool_input(&name, &input_json) {
                        Ok(input) => input,
                        Err(_) if stop_reason == Some(StopReason::MaxTokens) => {
                            tracing::warn!(
                                target: "topcoat_si::client",
                                tool = %name,
                                id = %id,
                                bytes = input_json.len(),
                                "dropped a tool call cut off at max_tokens"
                            );
                            return Ok(None);
                        }
                        Err(error) => return Err(error),
                    }
                } else {
                    input_value
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
            "msg_test",
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
        let text_delta = TextDelta::new("Hello");
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
            Ok(message_stop()),
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
            "msg_test",
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
            Ok(message_stop()),
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
    }

    /// Verifies that tool use with no delta events uses initial input_value.
    #[tokio::test]
    async fn tool_input_without_delta_uses_initial_value() {
        let usage = Usage::new(100, 0);
        let start_message = Message::new(
            "msg_test",
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
            Ok(message_stop()),
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
    }

    /// Verifies that container and diagnostics from message_start and
    /// context_management from message_delta reach the final message.
    #[tokio::test]
    async fn response_fields_are_carried_into_final_message() {
        use crate::{
            AppliedEdit, AppliedEditType, CacheDiagnostics, CacheMissReason, CacheMissType,
            ContainerInfo, ContextManagementResponse,
        };

        let mut start_message = Message::new(
            "msg_test",
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
                r#type: CacheMissType::SystemChanged,
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
                r#type: AppliedEditType::ClearToolUses20250919,
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
            Ok(message_stop()),
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
            CacheMissType::SystemChanged
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
            "msg_test",
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
        events.push(Ok(message_stop()));

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
            vec![ContentBlockDelta::TextDelta(TextDelta::new("ignored"))],
        )
        .await;
        assert_eq!(content.len(), 1);
        assert_eq!(content[0].as_unknown(), Some(&raw));
    }

    /// A `message_stop` with no `message_start` fails once, on the stream and on the receiver.
    #[tokio::test]
    async fn stop_without_start_fails_stream_and_receiver() {
        use futures::StreamExt;

        let events = vec![Ok(message_stop())];
        let (mut acc_stream, rx) = AccumulatingStream::new(stream::iter(events));
        assert!(acc_stream.next().await.unwrap().is_ok());
        let error = acc_stream.next().await.unwrap().unwrap_err();
        assert!(error.to_string().contains("without a message start event"));
        assert!(acc_stream.next().await.is_none());
        assert!(rx.await.unwrap().is_err());
    }

    /// A finished stream with no `message_start` is malformed output, but one that ends before
    /// `message_stop` was cut off.
    #[tokio::test]
    async fn stop_without_start_is_decode_and_early_end_is_interrupted() {
        use futures::StreamExt;

        let stop = vec![Ok(message_stop())];
        let (mut acc_stream, _rx) = AccumulatingStream::new(stream::iter(stop));
        assert!(acc_stream.next().await.unwrap().is_ok());
        let error = acc_stream.next().await.unwrap().unwrap_err();
        assert_eq!(error.kind(), &ErrorKind::Decode, "{error}");
        assert!(!error.is_retryable());

        let (mut acc_stream, _rx) = AccumulatingStream::new(message_stream(""));
        let error = acc_stream.next().await.unwrap().unwrap_err();
        assert_eq!(error.kind(), &ErrorKind::Interrupted, "{error}");
    }

    /// A stream that ends before `message_stop` yields no error of its own and never sends a
    /// message.
    #[tokio::test]
    async fn early_end_never_sends_a_message() {
        use futures::StreamExt;

        let events = vec![Ok(start_event()), Ok(stop_event(StopReason::EndTurn))];
        let (acc_stream, rx) = AccumulatingStream::new(stream::iter(events));
        let items: Vec<_> = acc_stream.collect().await;
        assert!(items.iter().all(Result::is_ok));
        assert!(rx.await.is_err(), "the channel closes without a message");
    }

    /// The decoded events of the server-sent `text`, as `Anthropic::stream` yields them.
    fn message_stream(
        text: &'static str,
    ) -> impl futures::Stream<Item = Result<MessageStreamEvent, Error>> + Send {
        crate::process_message_stream_sse(stream::iter(vec![Ok::<_, reqwest::Error>(
            bytes::Bytes::from_static(text.as_bytes()),
        )]))
    }

    fn message_stop() -> MessageStreamEvent {
        MessageStreamEvent::MessageStop(crate::MessageStopEvent::new())
    }

    /// `finalize_partial` on a running stream with no `message_start` is not malformed output.
    #[tokio::test]
    async fn finalize_partial_before_start_is_interrupted() {
        let (mut acc_stream, _rx) = AccumulatingStream::new(stream::iter(Vec::new()));
        let error = acc_stream.finalize_partial().unwrap_err();
        assert_eq!(error.kind(), &ErrorKind::Interrupted, "{error}");
    }

    fn start_event() -> MessageStreamEvent {
        MessageStreamEvent::MessageStart(MessageStartEvent::new(Message::new(
            "msg_test",
            Vec::new(),
            Model::Known(KnownModel::ClaudeSonnet55),
            Usage::new(1, 0),
        )))
    }

    fn tool_start_event() -> MessageStreamEvent {
        MessageStreamEvent::ContentBlockStart(ContentBlockStartEvent::new(
            ContentBlock::ToolUse(ToolUseBlock::new("toolu_1", "lookup", Value::Null)),
            0,
        ))
    }

    fn json_delta_event(index: usize, json: &str) -> MessageStreamEvent {
        MessageStreamEvent::ContentBlockDelta(ContentBlockDeltaEvent::new(
            ContentBlockDelta::InputJsonDelta(InputJsonDelta::new(json.to_string())),
            index,
        ))
    }

    fn stop_event(reason: StopReason) -> MessageStreamEvent {
        MessageStreamEvent::MessageDelta(MessageDeltaEvent::new(
            MessageDelta::new().with_stop_reason(reason),
            MessageDeltaUsage::new(1),
        ))
    }

    /// Drains a stream over `events`, returning its items and the final message.
    async fn drain(
        events: Vec<Result<MessageStreamEvent, Error>>,
    ) -> (
        Vec<Result<MessageStreamEvent, Error>>,
        Result<Message, Error>,
    ) {
        use futures::StreamExt;

        let (acc_stream, rx) = AccumulatingStream::new(stream::iter(events));
        let items: Vec<_> = acc_stream.collect().await;
        (items, rx.await.expect("channel closed"))
    }

    #[tokio::test]
    async fn tool_input_that_is_not_json_is_a_decode_error() {
        let events = vec![
            Ok(start_event()),
            Ok(tool_start_event()),
            Ok(json_delta_event(0, r#"{"query": SECRET-INPUT"#)),
            Ok(stop_event(StopReason::ToolUse)),
            Ok(message_stop()),
        ];
        let (items, message) = drain(events).await;
        let error = items.last().unwrap().as_ref().unwrap_err();
        assert_eq!(error.kind(), &ErrorKind::Decode, "{error}");
        assert!(!error.is_retryable());
        assert!(!error.to_string().contains("SECRET-INPUT"), "{error}");
        assert!(error.to_string().contains("lookup"), "{error}");
        assert!(message.is_err());
    }

    #[tokio::test]
    async fn tool_input_cut_off_at_max_tokens_is_dropped_and_logged() {
        let (_guard, targets) = crate::test_support::record_targets();
        let events = vec![
            Ok(start_event()),
            Ok(tool_start_event()),
            Ok(json_delta_event(0, r#"{"query": "ru"#)),
            Ok(stop_event(StopReason::MaxTokens)),
            Ok(message_stop()),
        ];
        let (items, message) = drain(events).await;
        assert!(items.iter().all(Result::is_ok));
        assert!(message.unwrap().content.is_empty());
        assert!(
            targets
                .lock()
                .unwrap()
                .contains(&"event topcoat_si::client".to_string()),
            "{:?}",
            targets.lock().unwrap()
        );
    }

    #[tokio::test]
    async fn delta_for_an_unknown_block_index_is_a_decode_error() {
        let events = vec![
            Ok(start_event()),
            Ok(tool_start_event()),
            Ok(json_delta_event(3, "{}")),
        ];
        let (items, message) = drain(events).await;
        let error = items.last().unwrap().as_ref().unwrap_err();
        assert_eq!(error.kind(), &ErrorKind::Decode, "{error}");
        assert!(error.to_string().contains("index 3"), "{error}");
        assert!(message.is_err());
    }

    #[tokio::test]
    async fn errors_say_how_many_events_were_delivered() {
        let failure = || Error::new(ErrorKind::Interrupted, "cut off");
        let (items, message) = drain(vec![Err(failure())]).await;
        let error = items[0].as_ref().unwrap_err();
        assert_eq!(error.events_delivered(), Some(0));
        assert_eq!(message.unwrap_err().events_delivered(), Some(0));

        let events = vec![Ok(start_event()), Ok(tool_start_event()), Err(failure())];
        let (items, _) = drain(events).await;
        let error = items[2].as_ref().unwrap_err();
        assert_eq!(error.events_delivered(), Some(2));
        assert_eq!(error.kind(), &ErrorKind::Interrupted);

        // Pings are not counted.
        let events = vec![
            Ok(MessageStreamEvent::Ping),
            Ok(start_event()),
            Ok(MessageStreamEvent::Ping),
            Err(failure()),
        ];
        let (items, _) = drain(events).await;
        assert_eq!(items[3].as_ref().unwrap_err().events_delivered(), Some(1));

        // A stream that ends early fails with the count too.
        let start = concat!(
            "event: message_start\n",
            r#"data: {"message":{"id":"msg_1","content":[],"model":"claude","role":"assistant","type":"message","usage":{"input_tokens":1,"output_tokens":1}}}"#,
            "\n\n",
        );
        let (acc_stream, _rx) = AccumulatingStream::new(message_stream(start));
        let items: Vec<_> = futures::StreamExt::collect(acc_stream).await;
        assert_eq!(items[1].as_ref().unwrap_err().events_delivered(), Some(1));
    }
}
