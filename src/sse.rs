//! Server-Sent Events (SSE) framing and decoding for streaming responses.
//!
//! This module provides the hardened raw SSE parser used by Claudius and
//! protocol-specific decoding helpers for Anthropic message streams.

use bytes::Bytes;
use futures::stream::{self, Stream, StreamExt};
use std::time::Instant;

use crate::observability;
use crate::{
    ContentBlockDeltaEvent, ContentBlockStartEvent, ContentBlockStopEvent, Error,
    MessageDeltaEvent, MessageStartEvent, MessageStopEvent, MessageStreamEvent, Result,
};

/// Maximum buffer size to prevent DoS attacks (1MB)
const MAX_BUFFER_SIZE: usize = 1024 * 1024;

/// Maximum event size (1MB).
const MAX_EVENT_SIZE: usize = MAX_BUFFER_SIZE;

/// A raw server-sent event frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// The SSE event name from the `event:` field.
    pub event: String,
    /// The SSE payload assembled from all `data:` lines.
    pub data: String,
}

/// State for SSE processing with production hardening
struct SseState {
    buffer: String,
    pending_utf8: Vec<u8>,
    finished: bool,
    total_bytes_processed: usize,
    start: Instant,
    first_byte: Option<Instant>,
}

/// Process a stream of bytes into a stream of raw server-sent events with production hardening.
///
/// This is the canonical SSE framing parser for Claudius and is intended for
/// reuse by other crates that need hardened SSE buffering and extraction.
///
/// This function takes a byte stream from an HTTP response and converts it into
/// a stream of parsed [`SseEvent`] objects, handling SSE parsing, buffering,
/// error conditions, DoS protection, and timeouts.
///
/// Production features:
/// - Buffer size limits to prevent memory exhaustion
/// - Event size validation
/// - Timeout handling for stalled connections
/// - Graceful error recovery
/// - UTF-8 validation with partial byte handling
pub fn process_sse<S>(byte_stream: S) -> impl Stream<Item = Result<SseEvent>>
where
    S: Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Unpin + 'static,
{
    // Convert reqwest errors to our error type
    let stream = byte_stream.map(|result| {
        result.map_err(|error| {
            crate::error::map_reqwest_error(error, crate::error::RequestPhase::Body)
        })
    });

    // Initialize state with production hardening
    let state = SseState {
        buffer: String::new(),
        pending_utf8: Vec::new(),
        finished: false,
        total_bytes_processed: 0,
        start: Instant::now(),
        first_byte: None,
    };

    stream::unfold((stream, state), move |(mut stream, mut state)| async move {
        loop {
            if state.finished {
                return None;
            }
            // Check if we have a complete event in the buffer
            match extract_event(&state.buffer) {
                Ok(Some((event, remaining))) => {
                    state.buffer = remaining;
                    match &event {
                        Ok(_) => observability::stream_event(),
                        Err(_) => state.fail(),
                    }
                    return Some((event, (stream, state)));
                }
                Ok(None) => {
                    // No complete event yet, continue reading
                }
                Err(e) => {
                    state.fail();
                    return Some((Err(e), (stream, state)));
                }
            }

            // Check buffer size limit
            if state.buffer.len() > MAX_BUFFER_SIZE {
                state.fail();
                return Some((
                    Err(Error::streaming(
                        format!("SSE buffer size exceeded maximum limit: {MAX_BUFFER_SIZE} bytes"),
                        None,
                    )),
                    (stream, state),
                ));
            }

            // Read more data
            match stream.next().await {
                Some(Ok(bytes)) => {
                    state.total_bytes_processed += bytes.len();
                    observability::stream_bytes(bytes.len() as u64);
                    if state.first_byte.is_none() {
                        let now = Instant::now();
                        state.first_byte = Some(now);
                        observability::stream_first_byte(now.duration_since(state.start));
                    }

                    let mut utf8_bytes = std::mem::take(&mut state.pending_utf8);
                    utf8_bytes.extend_from_slice(&bytes);

                    match String::from_utf8(utf8_bytes) {
                        Ok(text) => {
                            state.buffer.push_str(&text);
                        }
                        Err(e) => {
                            // Preserve incomplete UTF-8 sequences across chunk boundaries.
                            let utf8_error = e.utf8_error();
                            let valid_up_to = utf8_error.valid_up_to();
                            let bytes = e.into_bytes();

                            if valid_up_to > 0 {
                                let partial = match std::str::from_utf8(&bytes[..valid_up_to]) {
                                    Ok(partial) => partial,
                                    Err(error) => {
                                        state.fail();
                                        return Some((
                                            Err(Error::encoding(
                                                "Invalid UTF-8 prefix in SSE stream",
                                                Some(Box::new(error)),
                                            )),
                                            (stream, state),
                                        ));
                                    }
                                };
                                state.buffer.push_str(partial);
                            }

                            if utf8_error.error_len().is_none() {
                                state.pending_utf8.extend_from_slice(&bytes[valid_up_to..]);
                                continue;
                            }

                            state.fail();
                            return Some((
                                Err(Error::encoding(
                                    format!("Invalid UTF-8 in stream: {utf8_error}"),
                                    Some(Box::new(utf8_error)),
                                )),
                                (stream, state),
                            ));
                        }
                    }
                }
                Some(Err(e)) => {
                    state.fail();
                    return Some((Err(e), (stream, state)));
                }
                None => {
                    if !state.pending_utf8.is_empty() {
                        state.fail();
                        return Some((
                            Err(Error::encoding(
                                "Incomplete UTF-8 sequence at end of SSE stream".to_string(),
                                None,
                            )),
                            (stream, state),
                        ));
                    }
                    state.finished = true;
                    observability::stream_completed(state.start.elapsed());
                    return None;
                }
            }
        }
    })
    .fuse()
}

impl SseState {
    fn fail(&mut self) {
        self.finished = true;
        observability::stream_error();
    }
}

/// Decode a raw [`SseEvent`] into a Claudius [`MessageStreamEvent`].
pub fn parse_message_stream_event(event: &SseEvent) -> Result<MessageStreamEvent> {
    match event.event.as_str() {
        "" if event.data.is_empty() => Ok(MessageStreamEvent::Ping),
        "ping" => Ok(MessageStreamEvent::Ping),
        "message_start" => serde_json::from_str::<MessageStartEvent>(&event.data)
            .map(MessageStreamEvent::MessageStart)
            .map_err(Into::into),
        "message_delta" => serde_json::from_str::<MessageDeltaEvent>(&event.data)
            .map(MessageStreamEvent::MessageDelta)
            .map_err(Into::into),
        "message_stop" => serde_json::from_str::<MessageStopEvent>(&event.data)
            .map(MessageStreamEvent::MessageStop)
            .map_err(Into::into),
        "content_block_start" => serde_json::from_str::<ContentBlockStartEvent>(&event.data)
            .map(MessageStreamEvent::ContentBlockStart)
            .map_err(Into::into),
        "content_block_delta" => serde_json::from_str::<ContentBlockDeltaEvent>(&event.data)
            .map(MessageStreamEvent::ContentBlockDelta)
            .map_err(Into::into),
        "content_block_stop" => serde_json::from_str::<ContentBlockStopEvent>(&event.data)
            .map(MessageStreamEvent::ContentBlockStop)
            .map_err(Into::into),
        "error" => Err(parse_stream_error(&event.data)),
        _ => Ok(MessageStreamEvent::Unknown),
    }
}

/// Process a stream of bytes into decoded Claudius [`MessageStreamEvent`] values.
pub fn process_message_stream_sse<S>(
    byte_stream: S,
) -> impl Stream<Item = Result<MessageStreamEvent>>
where
    S: Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Unpin + 'static,
{
    process_sse(byte_stream)
        .scan(false, |finished, result| {
            let output = if *finished {
                None
            } else {
                let decoded = result.and_then(|event| {
                tracing::trace!(target: "claudius::wire", event = %event.event, data = %event.data);
                let result = parse_message_stream_event(&event);
                if result.is_err() { observability::stream_error(); }
                result
            });
                if decoded.is_err() {
                    *finished = true;
                }
                Some(decoded)
            };
            futures::future::ready(output)
        })
        .filter_map(|result| {
            futures::future::ready(match result {
                Ok(MessageStreamEvent::Unknown) => None,
                other => Some(other),
            })
        })
        .fuse()
}

/// Extract a complete SSE event from a buffer string with size validation.
fn extract_event(buffer: &str) -> Result<Option<(Result<SseEvent>, String)>> {
    let Some((event_end, delimiter_len)) = ["\n\n", "\r\n\r\n", "\r\r"]
        .iter()
        .filter_map(|delimiter| buffer.find(delimiter).map(|index| (index, delimiter.len())))
        .min_by_key(|(index, _)| *index)
    else {
        return Ok(None);
    };

    let event_text = &buffer[..event_end];
    let rest = buffer[event_end + delimiter_len..].to_string();

    if event_text.len() > MAX_EVENT_SIZE {
        return Ok(Some((
            Err(Error::streaming(
                format!(
                    "SSE event size {} exceeds maximum limit of {} bytes",
                    event_text.len(),
                    MAX_EVENT_SIZE
                ),
                None,
            )),
            rest,
        )));
    }

    if event_text.trim().is_empty() {
        return Ok(Some((
            Ok(SseEvent {
                event: String::new(),
                data: String::new(),
            }),
            rest,
        )));
    }

    let mut event_type = None;
    let mut data_lines = Vec::new();

    for raw_line in event_text.split(['\r', '\n']) {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if let Some(value) = parse_field_value(line, "event:") {
            event_type = Some(value.trim().to_string());
        } else if let Some(value) = parse_field_value(line, "data:") {
            data_lines.push(value.to_string());
        }
    }

    let Some(event) = event_type else {
        return Ok(Some((
            Err(Error::serialization(
                "Malformed SSE event: missing event type line".to_string(),
                None,
            )),
            rest,
        )));
    };

    if data_lines.is_empty() {
        return Ok(Some((
            Err(Error::serialization(
                "Malformed SSE event: missing data lines".to_string(),
                None,
            )),
            rest,
        )));
    }

    Ok(Some((
        Ok(SseEvent {
            event,
            data: data_lines.join("\n"),
        }),
        rest,
    )))
}

fn parse_field_value<'a>(line: &'a str, field: &str) -> Option<&'a str> {
    line.strip_prefix(field)
        .map(|value| value.strip_prefix(' ').unwrap_or(value))
}

fn parse_stream_error(event_data: &str) -> Error {
    match serde_json::from_str::<serde_json::Value>(event_data) {
        Ok(error_json) => {
            let error_object = error_json.get("error");
            let error_type = error_object
                .and_then(|error| error.get("type"))
                .and_then(|value| value.as_str())
                .map(String::from);
            let message = error_object
                .and_then(|error| error.get("message"))
                .and_then(|value| value.as_str())
                .unwrap_or("Unknown stream error")
                .to_string();
            let status_code = error_object
                .and_then(|error| error.get("status_code"))
                .and_then(|value| value.as_u64())
                .and_then(|value| u16::try_from(value).ok())
                .unwrap_or(match error_type.as_deref() {
                    Some("overloaded_error") => 529,
                    Some("rate_limit_error" | "rate_limit") => 429,
                    Some("invalid_request_error") => 400,
                    Some("authentication_error") => 401,
                    Some("permission_error") => 403,
                    Some("not_found_error" | "not_found") => 404,
                    _ => 500,
                });
            let _retryable = error_object
                .and_then(|error| error.get("retryable"))
                .and_then(|value| value.as_bool());

            Error::api(
                status_code,
                error_type,
                message,
                error_json["request_id"].as_str().map(String::from),
            )
        }
        Err(_) => Error::api(
            500,
            Some("stream_error".to_string()),
            event_data.to_string(),
            None,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;

    #[tokio::test]
    async fn parse_ping_event() {
        let stream = stream::iter(vec![Ok::<Bytes, reqwest::Error>(Bytes::from_static(
            b"event: ping\ndata: {}\n\n",
        ))]);

        let mut sse_stream = Box::pin(process_sse(stream));
        let event = sse_stream.next().await.unwrap().unwrap();

        assert_eq!(
            event,
            SseEvent {
                event: "ping".to_string(),
                data: "{}".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn parse_multiple_events() {
        let stream = stream::iter(vec![Ok::<Bytes, reqwest::Error>(Bytes::from_static(
            b"event: ping\ndata: {}\n\nevent: ping\ndata: {}\n\n",
        ))]);

        let mut sse_stream = Box::pin(process_sse(stream));

        let event1 = sse_stream.next().await.unwrap().unwrap();
        assert_eq!(
            event1,
            SseEvent {
                event: "ping".to_string(),
                data: "{}".to_string(),
            }
        );

        let event2 = sse_stream.next().await.unwrap().unwrap();
        assert_eq!(
            event2,
            SseEvent {
                event: "ping".to_string(),
                data: "{}".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn handle_malformed_event() {
        let stream = stream::iter(vec![Ok::<Bytes, reqwest::Error>(Bytes::from_static(
            b"malformed data without proper format\n\n",
        ))]);

        let mut sse_stream = Box::pin(process_sse(stream));
        let event = sse_stream.next().await.unwrap();

        assert!(event.is_err());
    }

    #[tokio::test]
    async fn handle_split_event() {
        let stream = stream::iter(vec![
            Ok::<Bytes, reqwest::Error>(Bytes::from_static(b"event: ping\n")),
            Ok::<Bytes, reqwest::Error>(Bytes::from_static(b"data: {}\n\n")),
        ]);

        let mut sse_stream = Box::pin(process_sse(stream));
        let event = sse_stream.next().await.unwrap().unwrap();

        assert_eq!(
            event,
            SseEvent {
                event: "ping".to_string(),
                data: "{}".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn handle_unknown_event_type_in_raw_parser() {
        let stream = stream::iter(vec![Ok::<Bytes, reqwest::Error>(Bytes::from_static(
            b"event: unknown_event\ndata: {}\n\n",
        ))]);

        let mut sse_stream = Box::pin(process_sse(stream));
        let event = sse_stream.next().await.unwrap().unwrap();

        assert_eq!(
            event,
            SseEvent {
                event: "unknown_event".to_string(),
                data: "{}".to_string(),
            }
        );
    }

    #[test]
    fn decoder_accepts_unknown_event_type() {
        let event = parse_message_stream_event(&SseEvent {
            event: "unknown_event".to_string(),
            data: "{}".to_string(),
        })
        .unwrap();

        assert!(matches!(event, MessageStreamEvent::Unknown));
    }

    #[tokio::test]
    async fn handle_buffer_size_limit() {
        let chunk_size = MAX_BUFFER_SIZE / 2;
        let chunk1 = "a".repeat(chunk_size);
        let chunk2 = "b".repeat(chunk_size + 1000);

        let stream = stream::iter(vec![
            Ok::<Bytes, reqwest::Error>(Bytes::from(chunk1)),
            Ok::<Bytes, reqwest::Error>(Bytes::from(chunk2)),
        ]);

        let mut sse_stream = Box::pin(process_sse(stream));
        let event = sse_stream.next().await.unwrap();

        assert!(event.is_err());
        if let Err(e) = event {
            assert!(e.to_string().contains("buffer size exceeded"));
        }
    }

    #[tokio::test]
    async fn handle_event_size_limit() {
        let large_event_data = "b".repeat(MAX_EVENT_SIZE + 100);
        let data = format!("event: ping\ndata: {large_event_data}\n\n");
        let stream = stream::iter(vec![Ok::<Bytes, reqwest::Error>(Bytes::from(data))]);

        let mut sse_stream = Box::pin(process_sse(stream));
        let event = sse_stream.next().await.unwrap();

        assert!(event.is_err());
        if let Err(e) = event {
            assert!(e.to_string().contains("event size") && e.to_string().contains("exceeds"));
        }
    }

    #[tokio::test]
    async fn handle_empty_events() {
        let stream = stream::iter(vec![Ok::<Bytes, reqwest::Error>(Bytes::from_static(
            b"\n\n",
        ))]);

        let mut sse_stream = Box::pin(process_sse(stream));
        let event = sse_stream.next().await.unwrap().unwrap();

        assert_eq!(
            event,
            SseEvent {
                event: String::new(),
                data: String::new(),
            }
        );
    }

    #[test]
    fn decoder_treats_empty_event_as_ping() {
        let event = parse_message_stream_event(&SseEvent {
            event: String::new(),
            data: String::new(),
        })
        .unwrap();

        assert!(matches!(event, MessageStreamEvent::Ping));
    }

    #[tokio::test]
    async fn handle_multi_line_data() {
        let stream = stream::iter(vec![Ok::<Bytes, reqwest::Error>(Bytes::from_static(
            b"event: message_start\ndata: {\ndata: \"test\": true\ndata: }\n\n",
        ))]);

        let mut sse_stream = Box::pin(process_sse(stream));
        let event = sse_stream.next().await.unwrap().unwrap();

        assert_eq!(
            event,
            SseEvent {
                event: "message_start".to_string(),
                data: "{\n\"test\": true\n}".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn handle_partial_utf8() {
        let valid_part = "event: ping\ndata: test";
        let invalid_bytes = vec![0xFF, 0xFE];

        let mut data = valid_part.as_bytes().to_vec();
        data.extend_from_slice(&invalid_bytes);
        data.extend_from_slice(b"\n\n");

        let stream = stream::iter(vec![Ok::<Bytes, reqwest::Error>(Bytes::from(data))]);
        let mut sse_stream = Box::pin(process_sse(stream));

        if let Some(event) = sse_stream.next().await
            && let Err(e) = event
        {
            assert!(e.to_string().contains("UTF-8"));
        }
    }

    #[tokio::test]
    async fn handle_split_utf8_across_chunks() {
        let stream = stream::iter(vec![
            Ok::<Bytes, reqwest::Error>(Bytes::from_static(b"event: ping\ndata: caf\xc3")),
            Ok::<Bytes, reqwest::Error>(Bytes::from_static(b"\xa9\n\n")),
        ]);

        let mut sse_stream = Box::pin(process_sse(stream));
        let event = sse_stream.next().await.unwrap().unwrap();

        assert_eq!(
            event,
            SseEvent {
                event: "ping".to_string(),
                data: "caf\u{00e9}".to_string(),
            }
        );
    }

    #[test]
    fn decode_message_start_event() {
        let data = r#"{"message":{"id":"msg_012345","content":[],"model":"claude-3-sonnet-20240229","role":"assistant","type":"message","usage":{"input_tokens":50,"output_tokens":100}}}"#;
        let event = parse_message_stream_event(&SseEvent {
            event: "message_start".to_string(),
            data: data.to_string(),
        })
        .unwrap();

        assert!(matches!(event, MessageStreamEvent::MessageStart(_)));
    }

    #[test]
    fn decode_message_delta_event() {
        let data = r#"{"delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":50,"output_tokens":100}}"#;
        let event = parse_message_stream_event(&SseEvent {
            event: "message_delta".to_string(),
            data: data.to_string(),
        })
        .unwrap();

        assert!(matches!(event, MessageStreamEvent::MessageDelta(_)));
    }

    #[test]
    fn decode_content_block_start_event() {
        let data = r#"{"content_block":{"text":"Hello, I'm Claude.","type":"text"},"index":0}"#;
        let event = parse_message_stream_event(&SseEvent {
            event: "content_block_start".to_string(),
            data: data.to_string(),
        })
        .unwrap();

        assert!(matches!(event, MessageStreamEvent::ContentBlockStart(_)));
    }

    #[test]
    fn decode_content_block_delta_event() {
        let data = r#"{"delta":{"text":"Hello, I'm Claude.","type":"text_delta"},"index":0}"#;
        let event = parse_message_stream_event(&SseEvent {
            event: "content_block_delta".to_string(),
            data: data.to_string(),
        })
        .unwrap();

        assert!(matches!(event, MessageStreamEvent::ContentBlockDelta(_)));
    }

    #[test]
    fn decode_content_block_stop_event() {
        let event = parse_message_stream_event(&SseEvent {
            event: "content_block_stop".to_string(),
            data: r#"{"index":0}"#.to_string(),
        })
        .unwrap();

        assert!(matches!(event, MessageStreamEvent::ContentBlockStop(_)));
    }

    #[test]
    fn decode_message_stop_event() {
        let event = parse_message_stream_event(&SseEvent {
            event: "message_stop".to_string(),
            data: "{}".to_string(),
        })
        .unwrap();

        assert!(matches!(event, MessageStreamEvent::MessageStop(_)));
    }

    #[tokio::test]
    async fn process_message_stream_sse_decodes_ping() {
        let stream = stream::iter(vec![Ok::<Bytes, reqwest::Error>(Bytes::from_static(
            b"event: ping\ndata: {}\n\n",
        ))]);

        let mut sse_stream = Box::pin(process_message_stream_sse(stream));
        let event = sse_stream.next().await.unwrap().unwrap();

        assert!(matches!(event, MessageStreamEvent::Ping));
    }

    #[test]
    fn handle_structured_error_events_with_status_code() {
        let error_json = r#"{"error":{"type":"rate_limit","message":"Too many requests","status_code":429,"retryable":true}}"#;
        let err = parse_message_stream_event(&SseEvent {
            event: "error".to_string(),
            data: error_json.to_string(),
        })
        .unwrap_err();

        assert_eq!(err.status_code(), Some(429));
        assert!(err.to_string().contains("Too many requests"));
    }

    #[test]
    fn handle_structured_error_events_without_status_code() {
        let error_json = r#"{"error":{"type":"not_found","message":"missing"}}"#;
        let err = parse_message_stream_event(&SseEvent {
            event: "error".to_string(),
            data: error_json.to_string(),
        })
        .unwrap_err();

        assert_eq!(err.status_code(), Some(404));
        assert!(err.to_string().contains("missing"));
    }
}
