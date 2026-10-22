//! Server-Sent Events (SSE) framing and decoding for streaming responses.
//!
//! This module provides the raw SSE parser used by Topcoat SI and
//! protocol-specific decoding helpers for Anthropic message streams.

use bytes::Bytes;
use futures::stream::{self, Stream, StreamExt};
use serde::Deserialize;
use std::time::Instant;

use crate::error::{RequestPhase, Service, describe_json_error};
use crate::observability;
use crate::{
    ContentBlockDeltaEvent, ContentBlockStartEvent, ContentBlockStopEvent, Error, ErrorKind,
    ErrorType, MessageDeltaEvent, MessageStartEvent, MessageStopEvent, MessageStreamEvent, Result,
};

/// Maximum buffer and event size to prevent DoS attacks (1MB)
const MAX_BUFFER_SIZE: usize = 1024 * 1024;

/// A raw server-sent event frame.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SseEvent {
    /// The SSE event name from the `event:` field.
    pub event: String,
    /// The SSE payload assembled from all `data:` lines.
    pub data: String,
}

impl SseEvent {
    /// Builds a frame from its `event:` name and its `data:` payload.
    #[must_use]
    pub fn new(event: impl Into<String>, data: impl Into<String>) -> Self {
        Self {
            event: event.into(),
            data: data.into(),
        }
    }
}

/// The parser state: buffered text, a split UTF-8 sequence carried over, and timing.
struct SseState {
    buffer: String,
    pending_utf8: Vec<u8>,
    finished: bool,
    start: Instant,
    first_byte: Option<Instant>,
}

/// Process a stream of bytes into a stream of raw server-sent events.
///
/// This function takes a byte stream from an HTTP response, buffers it, and
/// yields each complete frame as an [`SseEvent`].
///
/// - The buffer and each event are capped at 1MB to bound memory use.
/// - UTF-8 sequences split across chunks are carried over to the next chunk.
/// - Invalid UTF-8, an oversized buffer or event, a malformed frame, or a
///   transport error yields one error, after which the stream ends.
pub fn process_sse<S>(byte_stream: S) -> impl Stream<Item = Result<SseEvent>>
where
    S: Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Unpin + 'static,
{
    // Convert reqwest errors to our error type
    let stream = byte_stream.map(|result| {
        result.map_err(|error| crate::error::map_reqwest_error(error, RequestPhase::Body))
    });

    let state = SseState {
        buffer: String::new(),
        pending_utf8: Vec::new(),
        finished: false,
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
                    let Some(event) = event else {
                        // Nothing to dispatch, such as a comment-only frame.
                        continue;
                    };
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
                    Err(Error::new(
                        ErrorKind::Decode,
                        format!("SSE buffer size exceeded maximum limit: {MAX_BUFFER_SIZE} bytes"),
                    )),
                    (stream, state),
                ));
            }

            // Read more data
            match stream.next().await {
                Some(Ok(bytes)) => {
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
                                // `valid_up_to` marks a prefix that is valid UTF-8.
                                state
                                    .buffer
                                    .push_str(&String::from_utf8_lossy(&bytes[..valid_up_to]));
                            }

                            if utf8_error.error_len().is_none() {
                                state.pending_utf8.extend_from_slice(&bytes[valid_up_to..]);
                                continue;
                            }

                            state.fail();
                            return Some((
                                Err(Error::new(ErrorKind::Decode, "Invalid UTF-8 in SSE stream")
                                    .with_source(utf8_error)),
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
                            Err(Error::new(
                                ErrorKind::Decode,
                                "Incomplete UTF-8 sequence at end of SSE stream",
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

/// Decode a raw [`SseEvent`] into a Topcoat SI [`MessageStreamEvent`].
pub fn parse_message_stream_event(event: &SseEvent) -> Result<MessageStreamEvent> {
    match event.event.as_str() {
        "" if event.data.is_empty() => Ok(MessageStreamEvent::Ping),
        "ping" => Ok(MessageStreamEvent::Ping),
        "message_start" => serde_json::from_str::<MessageStartEvent>(&event.data)
            .map(MessageStreamEvent::MessageStart)
            .map_err(|error| event_error(event, error)),
        "message_delta" => serde_json::from_str::<MessageDeltaEvent>(&event.data)
            .map(MessageStreamEvent::MessageDelta)
            .map_err(|error| event_error(event, error)),
        "message_stop" => serde_json::from_str::<MessageStopEvent>(&event.data)
            .map(MessageStreamEvent::MessageStop)
            .map_err(|error| event_error(event, error)),
        "content_block_start" => serde_json::from_str::<ContentBlockStartEvent>(&event.data)
            .map(MessageStreamEvent::ContentBlockStart)
            .map_err(|error| event_error(event, error)),
        "content_block_delta" => serde_json::from_str::<ContentBlockDeltaEvent>(&event.data)
            .map(MessageStreamEvent::ContentBlockDelta)
            .map_err(|error| event_error(event, error)),
        "content_block_stop" => serde_json::from_str::<ContentBlockStopEvent>(&event.data)
            .map(MessageStreamEvent::ContentBlockStop)
            .map_err(|error| event_error(event, error)),
        "error" => Err(parse_stream_error(&event.data)),
        name => {
            // Dropped by `process_message_stream_sse`; the event name is enough to spot it.
            crate::types::warn_unknown("MessageStreamEvent", name);
            Ok(MessageStreamEvent::Unknown)
        }
    }
}

/// Process a stream of bytes into decoded Topcoat SI [`MessageStreamEvent`] values.
///
/// An error item carries [`Error::events_delivered`]: the number of events other than pings
/// yielded before it. `Some(0)` means sending the request again is clean. A stream whose bytes
/// end before `message_stop` yields an [`ErrorKind::Interrupted`] error last.
pub fn process_message_stream_sse<S>(
    byte_stream: S,
) -> impl Stream<Item = Result<MessageStreamEvent>>
where
    S: Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Unpin + 'static,
{
    let events = process_sse(byte_stream)
        .scan(false, |finished, result| {
            let output = if *finished {
                None
            } else {
                let decoded = decode(result);
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
        });
    let state = MessageStreamState {
        events: Box::pin(events),
        delivered: 0,
        stopped: false,
        finished: false,
    };
    stream::unfold(state, |mut state| async move {
        if state.finished {
            return None;
        }
        let item = match state.events.next().await {
            Some(Ok(event)) => {
                match event {
                    MessageStreamEvent::Ping => {}
                    MessageStreamEvent::MessageStop(_) => {
                        state.stopped = true;
                        state.delivered += 1;
                    }
                    _ => state.delivered += 1,
                }
                Ok(event)
            }
            Some(Err(error)) => {
                state.finished = true;
                Err(error.with_events_delivered(state.delivered))
            }
            None if state.stopped => return None,
            None => {
                state.finished = true;
                Err(
                    Error::new(ErrorKind::Interrupted, "stream ended before message_stop")
                        .with_events_delivered(state.delivered),
                )
            }
        };
        Some((item, state))
    })
    .fuse()
}

/// The decoded events of a message stream, how many were not pings, and whether `message_stop`
/// arrived.
struct MessageStreamState<E> {
    events: std::pin::Pin<Box<E>>,
    delivered: u64,
    stopped: bool,
    finished: bool,
}

/// Decode one framed SSE event, counting a stream error only when decoding fails.
///
/// Framing errors pass through uncounted: `process_sse` already counted them.
fn decode(result: Result<SseEvent>) -> Result<MessageStreamEvent> {
    let event = result?;
    tracing::trace!(target: "topcoat_si::wire", event = %event.event, data = %event.data);
    let decoded = parse_message_stream_event(&event);
    if decoded.is_err() {
        observability::stream_error();
    }
    decoded
}

/// Extract a complete SSE event from a buffer string with size validation.
///
/// The event is `None` when the frame has nothing to dispatch: only comments or fields other
/// than `data`.
fn extract_event(buffer: &str) -> Result<Option<(Option<Result<SseEvent>>, String)>> {
    let Some((event_end, delimiter_len)) = ["\n\n", "\r\n\r\n", "\r\r"]
        .iter()
        .filter_map(|delimiter| buffer.find(delimiter).map(|index| (index, delimiter.len())))
        .min_by_key(|(index, _)| *index)
    else {
        return Ok(None);
    };

    let event_text = &buffer[..event_end];
    let rest = buffer[event_end + delimiter_len..].to_string();

    if event_text.len() > MAX_BUFFER_SIZE {
        return Ok(Some((
            Some(Err(Error::new(
                ErrorKind::Decode,
                format!(
                    "SSE event size {} exceeds maximum limit of {} bytes",
                    event_text.len(),
                    MAX_BUFFER_SIZE
                ),
            ))),
            rest,
        )));
    }

    if event_text.trim().is_empty() {
        return Ok(Some((
            Some(Ok(SseEvent {
                event: String::new(),
                data: String::new(),
            })),
            rest,
        )));
    }

    let mut event_type = None;
    let mut data_lines = Vec::new();

    for raw_line in event_text.split(['\r', '\n']) {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.is_empty() || line.starts_with(':') {
            // A comment, such as a `: keepalive` line.
            continue;
        }
        match parse_field(line) {
            ("event", value) => {
                event_type = Some(value.trim().to_string()).filter(|name| !name.is_empty());
            }
            ("data", value) => data_lines.push(value.to_string()),
            // Any other field, such as `id` or `retry`, is ignored.
            _ => {}
        }
    }

    if data_lines.is_empty() {
        // The SSE spec dispatches nothing for a frame without data.
        return Ok(Some((None, rest)));
    }

    Ok(Some((
        Some(Ok(SseEvent {
            event: event_type.unwrap_or_else(|| "message".to_string()),
            data: data_lines.join("\n"),
        })),
        rest,
    )))
}

/// Splits an SSE line into its field name and value. A line without a colon is a field with an
/// empty value; one space after the colon is dropped.
fn parse_field(line: &str) -> (&str, &str) {
    match line.split_once(':') {
        Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
        None => (line, ""),
    }
}

/// The error for an SSE event whose payload does not decode. It gives the byte count and where
/// parsing failed, never the payload, and keeps no source, since serde's text quotes the payload.
fn event_error(event: &SseEvent, error: serde_json::Error) -> Error {
    Error::new(
        ErrorKind::Decode,
        format!(
            "could not decode the {} event ({} bytes): {}",
            event.event,
            event.data.len(),
            describe_json_error(&error)
        ),
    )
}

/// The most payload text a stream error message copies, in bytes.
const MAX_ERROR_TEXT_BYTES: usize = 300;

/// `text` cut to [`MAX_ERROR_TEXT_BYTES`] at a character boundary.
fn capped(text: &str) -> String {
    if text.len() <= MAX_ERROR_TEXT_BYTES {
        return text.to_string();
    }
    let mut end = MAX_ERROR_TEXT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

/// Builds the error for an `error` event inside a stream.
///
/// The event has no HTTP status. A payload that is not JSON, or has no `error` object or message,
/// still becomes an [`ErrorKind::Api`] error with no type, since the service did send an `error`
/// event; the message then carries the start of the payload.
fn parse_stream_error(event_data: &str) -> Error {
    let payload = serde_json::from_str::<serde_json::Value>(event_data).ok();
    let error_object = payload.as_ref().and_then(|payload| payload.get("error"));
    let text = |key: &str| {
        error_object
            .and_then(|error| error.get(key))
            .and_then(serde_json::Value::as_str)
    };
    let message = text("message")
        .map(capped)
        .unwrap_or_else(|| format!("unrecognized error event: {}", capped(event_data)));
    let kind = ErrorKind::Api {
        service: Service::Anthropic,
        status: None,
        error_type: error_object
            .and_then(|error| error.get("type"))
            .and_then(|value| ErrorType::deserialize(value).ok()),
        error_code: None,
        retry_after: None,
        retryable: error_object
            .and_then(|error| error.get("retryable"))
            .and_then(serde_json::Value::as_bool),
    };
    let request_id = payload
        .as_ref()
        .and_then(|payload| payload.get("request_id"))
        .and_then(serde_json::Value::as_str)
        .map(String::from);
    Error::new(kind, message).with_request_id(request_id)
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

        // A line without a colon is a field with an empty value; this unknown field is ignored,
        // and a frame without data dispatches nothing.
        assert!(sse_stream.next().await.is_none());
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
        let large_event_data = "b".repeat(MAX_BUFFER_SIZE + 100);
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
        let invalid_bytes = vec![0xff, 0xfe];

        let mut data = valid_part.as_bytes().to_vec();
        data.extend_from_slice(&invalid_bytes);
        data.extend_from_slice(b"\n\n");

        let stream = stream::iter(vec![Ok::<Bytes, reqwest::Error>(Bytes::from(data))]);
        let mut sse_stream = Box::pin(process_sse(stream));

        let error = sse_stream.next().await.unwrap().unwrap_err();
        assert_eq!(error.kind(), &ErrorKind::Decode);
        assert!(error.to_string().contains("UTF-8"));
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
    fn structured_error_event_with_a_status_code_has_no_status() {
        let error_json = r#"{"error":{"type":"rate_limit","message":"Too many requests","status_code":429,"retryable":true}}"#;
        let err = parse_message_stream_event(&SseEvent {
            event: "error".to_string(),
            data: error_json.to_string(),
        })
        .unwrap_err();

        assert_eq!(err.status(), None);
        assert!(err.to_string().contains("Too many requests"));
    }

    #[test]
    fn error_event_type_becomes_a_typed_error_type() {
        let parse = |data: &str| api_parts(&error_event(data)).1;
        let (code, warnings) = crate::test_support::unknown_warnings(|| {
            parse(r#"{"error":{"type":"overloaded_error","message":"busy"}}"#)
        });
        assert_eq!(code, Some(ErrorType::OverloadedError));
        assert_eq!(warnings, Vec::<String>::new());

        let (codes, warnings) = crate::test_support::unknown_warnings(|| {
            [
                parse(r#"{"error":{"type":"kettle_error","message":"hot"}}"#),
                parse(r#"{"error":{"type":"kettle_error","message":"hot"}}"#),
            ]
        });
        assert_eq!(codes, [Some(ErrorType::Unknown), Some(ErrorType::Unknown)]);
        assert_eq!(warnings, ["rust_type=ErrorType tag=kettle_error"]);

        assert_eq!(parse("not json"), None);
    }

    #[test]
    fn stream_error_retryability_follows_its_type() {
        // (type, parsed type, retryable)
        let cases = [
            (
                "invalid_request_error",
                ErrorType::InvalidRequestError,
                false,
            ),
            (
                "authentication_error",
                ErrorType::AuthenticationError,
                false,
            ),
            ("billing_error", ErrorType::BillingError, false),
            ("permission_error", ErrorType::PermissionError, false),
            ("not_found_error", ErrorType::NotFoundError, false),
            ("conflict_error", ErrorType::ConflictError, false),
            ("request_too_large", ErrorType::RequestTooLarge, false),
            ("rate_limit_error", ErrorType::RateLimitError, true),
            ("api_error", ErrorType::ApiError, true),
            ("timeout_error", ErrorType::TimeoutError, true),
            ("overloaded_error", ErrorType::OverloadedError, true),
            // The old undocumented aliases are unknown types now.
            ("rate_limit", ErrorType::Unknown, false),
            ("not_found", ErrorType::Unknown, false),
        ];
        for (wire, error_type, retryable) in cases {
            let data = format!(r#"{{"error":{{"type":"{wire}","message":"m"}}}}"#);
            let err = error_event(&data);
            assert_eq!(api_parts(&err), (None, Some(error_type), None), "{wire}");
            assert_eq!(err.is_retryable(), retryable, "{wire}");
        }
    }

    #[test]
    fn structured_error_event_without_a_status_code_has_no_status() {
        let error_json = r#"{"error":{"type":"not_found_error","message":"missing"}}"#;
        let err = parse_message_stream_event(&SseEvent {
            event: "error".to_string(),
            data: error_json.to_string(),
        })
        .unwrap_err();

        assert_eq!(err.status(), None);
        assert!(err.to_string().contains("missing"));
    }

    fn error_event(data: &str) -> Error {
        parse_message_stream_event(&SseEvent::new("error", data)).unwrap_err()
    }

    fn api_parts(error: &Error) -> (Option<u16>, Option<ErrorType>, Option<bool>) {
        match error.kind() {
            ErrorKind::Api {
                status,
                error_type,
                retryable,
                ..
            } => (*status, *error_type, *retryable),
            other => panic!("expected an Api error, got {other:?}"),
        }
    }

    #[test]
    fn stream_error_events_have_no_status_and_follow_their_type() {
        let error = error_event(
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"},"request_id":"req_7"}"#,
        );
        assert_eq!(
            api_parts(&error),
            (None, Some(ErrorType::OverloadedError), None)
        );
        assert!(error.is_retryable());
        assert_eq!(error.request_id(), Some("req_7"));
        assert_eq!(
            error.to_string(),
            "Anthropic API error: Overloaded (in stream, overloaded_error, request req_7)"
        );

        let error = error_event(r#"{"error":{"type":"invalid_request_error","message":"bad"}}"#);
        assert_eq!(api_parts(&error).0, None);
        assert!(!error.is_retryable());
    }

    #[test]
    fn stream_error_events_honor_the_retryable_flag() {
        let error = error_event(
            r#"{"error":{"type":"overloaded_error","message":"full","retryable":false}}"#,
        );
        assert_eq!(api_parts(&error).2, Some(false));
        assert!(!error.is_retryable());

        let error = error_event(
            r#"{"error":{"type":"invalid_request_error","message":"flaky","retryable":true}}"#,
        );
        assert!(error.is_retryable());
    }

    #[test]
    fn unparseable_and_unknown_error_payloads_are_not_a_500() {
        for data in [
            "not json at all",
            r#"{"surprise":true}"#,
            r#"{"error":"flat"}"#,
        ] {
            let error = error_event(data);
            assert_eq!(api_parts(&error), (None, None, None), "{data}");
            assert_eq!(error.status(), None, "{data}");
            assert!(!error.is_retryable(), "{data}");
            assert!(error.to_string().contains(data), "{error}");
        }
    }

    #[test]
    fn error_payload_text_is_capped_at_a_character_boundary() {
        // Two-byte characters, so the 300-byte cap falls inside one when the count is odd.
        let long = "\u{e9}".repeat(MAX_ERROR_TEXT_BYTES);
        let error = error_event(&format!(
            r#"{{"error":{{"type":"api_error","message":"{long}"}}}}"#
        ));
        assert!(
            error.message().len() <= MAX_ERROR_TEXT_BYTES + 3,
            "{}",
            error.message()
        );
        assert!(error.message().ends_with("..."));

        let error = error_event(&"x".repeat(MAX_BUFFER_SIZE / 2));
        assert!(error.message().len() < 400, "{}", error.message().len());

        let odd = format!("a{}", "\u{e9}".repeat(MAX_ERROR_TEXT_BYTES));
        // 1 + 2 * 149 = 299 bytes fit, and the 300th byte splits a character.
        assert_eq!(capped(&odd).len(), 299 + 3);
    }

    async fn frames(bytes: &'static [u8]) -> Vec<Result<SseEvent>> {
        let stream = stream::iter(vec![Ok::<Bytes, reqwest::Error>(Bytes::from_static(bytes))]);
        process_sse(stream).collect().await
    }

    #[tokio::test]
    async fn comment_lines_are_skipped() {
        let frames = frames(b": keepalive\n\nevent: ping\n: note\ndata: {}\n\n").await;
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].as_ref().unwrap(), &SseEvent::new("ping", "{}"));
    }

    #[tokio::test]
    async fn comment_only_frames_yield_nothing_in_a_message_stream() {
        let stream = stream::iter(vec![Ok::<Bytes, reqwest::Error>(Bytes::from_static(
            b": keepalive\n\n: another\n\nevent: message_stop\ndata: {}\n\n",
        ))]);
        let events: Vec<_> = process_message_stream_sse(stream).collect().await;
        assert!(
            matches!(events[..], [Ok(MessageStreamEvent::MessageStop(_))]),
            "{events:?}"
        );
    }

    #[tokio::test]
    async fn frames_with_fields_but_no_data_are_dropped() {
        let frames = frames(b"event: foo\n\nid: 3\nevent: bar\n\nevent: ping\ndata: {}\n\n").await;
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].as_ref().unwrap(), &SseEvent::new("ping", "{}"));
    }

    #[tokio::test]
    async fn bare_field_names_have_empty_values() {
        let frames = frames(b"data\n\nevent\ndata: {}\n\ndata\ndata: x\nretry\n\n").await;
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].as_ref().unwrap(), &SseEvent::new("message", ""));
        assert_eq!(frames[1].as_ref().unwrap(), &SseEvent::new("message", "{}"));
        assert_eq!(
            frames[2].as_ref().unwrap(),
            &SseEvent::new("message", "\nx")
        );
    }

    #[tokio::test]
    async fn unknown_fields_retry_and_id_are_ignored() {
        let frames =
            frames(b"retry: 3000\nid: 7\nflavor: mint\nevent: ping\ndata: {}\n\nretry: 10\n\n")
                .await;
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].as_ref().unwrap(), &SseEvent::new("ping", "{}"));
    }

    #[tokio::test]
    async fn event_name_defaults_to_message() {
        let frames = frames(b"data: {}\n\nevent:\ndata: {}\n\n").await;
        assert_eq!(frames.len(), 2);
        for frame in frames {
            assert_eq!(frame.unwrap(), SseEvent::new("message", "{}"));
        }
    }

    #[tokio::test]
    async fn legal_sse_does_not_end_a_message_stream() {
        let stream = stream::iter(vec![Ok::<Bytes, reqwest::Error>(Bytes::from_static(
            b": keepalive\n\nretry: 5\n\ndata: {}\n\nevent: ping\ndata: {}\n\nevent: message_stop\ndata: {}\n\n",
        ))]);
        let events: Vec<_> = process_message_stream_sse(stream).collect().await;
        assert!(events.iter().all(Result::is_ok), "{events:?}");
        assert!(matches!(
            events[..],
            [
                Ok(MessageStreamEvent::Ping),
                Ok(MessageStreamEvent::MessageStop(_))
            ]
        ));
    }

    #[tokio::test]
    async fn message_delta_with_stop_reason_but_no_message_stop_yields_interrupted() {
        let text = concat!(
            "event: message_start\n",
            r#"data: {"message":{"id":"msg_1","content":[],"model":"claude","role":"assistant","type":"message","usage":{"input_tokens":1,"output_tokens":1}}}"#,
            "\n\nevent: ping\ndata: {}\n\nevent: message_delta\n",
            r#"data: {"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}"#,
            "\n\n",
        );
        for (bytes, delivered) in [(text, 2), ("", 0)] {
            let stream = stream::iter(vec![Ok::<Bytes, reqwest::Error>(Bytes::from(bytes))]);
            let items: Vec<_> = process_message_stream_sse(stream).collect().await;
            let error = items.last().unwrap().as_ref().unwrap_err();
            assert_eq!(error.kind(), &ErrorKind::Interrupted, "{error}");
            assert!(error.is_retryable());
            assert_eq!(error.events_delivered(), Some(delivered));
            assert!(items[..items.len() - 1].iter().all(Result::is_ok));
        }
    }

    #[tokio::test]
    async fn stream_errors_say_how_many_events_were_delivered() {
        let bad = "event: content_block_stop\ndata: nope\n\n";
        let start = r#"event: message_start
data: {"message":{"id":"msg_1","content":[],"model":"claude","role":"assistant","type":"message","usage":{"input_tokens":1,"output_tokens":1}}}

"#;
        let ping = "event: ping\ndata: {}\n\n";
        let cases = [
            (bad.to_string(), 0),
            // Pings are not counted.
            (format!("{ping}{ping}{bad}"), 0),
            (format!("{start}{ping}{bad}"), 1),
        ];
        for (text, delivered) in cases {
            let stream = stream::iter(vec![Ok::<Bytes, reqwest::Error>(Bytes::from(text))]);
            let items: Vec<_> = process_message_stream_sse(stream).collect().await;
            let error = items.last().unwrap().as_ref().unwrap_err();
            assert_eq!(error.events_delivered(), Some(delivered), "{error}");
            assert_eq!(error.kind(), &ErrorKind::Decode);
        }
    }

    #[test]
    fn undecodable_event_payloads_do_not_leak_into_errors() {
        let cases = [
            ("message_start", r#"{"message":"SECRET-PAYLOAD"}"#),
            ("content_block_delta", r#"SECRET-PAYLOAD"#),
            ("message_delta", r#"{"delta":"SECRET-PAYLOAD"}"#),
        ];
        for (name, data) in cases {
            let error = parse_message_stream_event(&SseEvent::new(name, data)).unwrap_err();
            assert_eq!(error.kind(), &ErrorKind::Decode);
            assert!(!error.to_string().contains("SECRET-PAYLOAD"), "{error}");
            assert!(std::error::Error::source(&error).is_none(), "{error:?}");
            assert!(
                error.to_string().contains(&format!("{} bytes", data.len())),
                "{error}"
            );
        }
    }
}
