use bytes::Bytes;
use claudius::*;
use futures::{StreamExt, stream};
use serde_json::json;

fn start() -> MessageStreamEvent {
    MessageStreamEvent::MessageStart(MessageStartEvent::new(Message::new(
        "msg_test".into(),
        vec![],
        Model::Custom("claude-test".into()),
        Usage::new(10, 0),
    )))
}
fn text_start() -> MessageStreamEvent {
    MessageStreamEvent::ContentBlockStart(ContentBlockStartEvent::new(
        ContentBlock::Text(TextBlock::new("partial")),
        0,
    ))
}

#[tokio::test]
async fn server_tool_json_deltas_must_be_accumulated() {
    let events = vec![
        Ok(start()),
        Ok(MessageStreamEvent::ContentBlockStart(
            ContentBlockStartEvent::new(
                ContentBlock::ServerToolUse(ServerToolUseBlock::new("srvtoolu_test", json!({}))),
                0,
            ),
        )),
        Ok(MessageStreamEvent::ContentBlockDelta(
            ContentBlockDeltaEvent::new(
                ContentBlockDelta::InputJsonDelta(InputJsonDelta::new(
                    r#"{"query":"weather NYC today"}"#.into(),
                )),
                0,
            ),
        )),
        Ok(MessageStreamEvent::ContentBlockStop(
            ContentBlockStopEvent::new(0),
        )),
        Ok(MessageStreamEvent::MessageDelta(MessageDeltaEvent::new(
            MessageDelta::new().with_stop_reason(StopReason::EndTurn),
            MessageDeltaUsage::new(20),
        ))),
        Ok(MessageStreamEvent::MessageStop(MessageStopEvent::new())),
    ];
    let (mut events, message) = AccumulatingStream::new(stream::iter(events));
    while let Some(event) = events.next().await {
        event.unwrap();
    }
    let message = message.await.unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(&message.content[0]).unwrap()["input"],
        json!({"query":"weather NYC today"})
    );
}

#[tokio::test]
async fn eof_without_message_stop_must_not_succeed() {
    let (mut events, message) =
        AccumulatingStream::new(stream::iter(vec![Ok(start()), Ok(text_start())]));
    let mut errors = 0;
    while let Some(event) = events.next().await {
        errors += usize::from(event.is_err());
    }
    assert_eq!(errors, 1);
    assert!(events.next().await.is_none());
    let result = message.await.unwrap();
    assert!(
        result.is_err(),
        "truncated response was accepted: {result:?}"
    );
}

#[tokio::test]
async fn stream_error_must_reach_final_message_receiver() {
    let (mut events, message) = AccumulatingStream::new(stream::iter(vec![
        Ok(start()),
        Ok(text_start()),
        Err(Error::timeout("connection stalled", None)),
    ]));
    while events.next().await.is_some() {}
    let result = message.await.unwrap();
    assert!(result.is_err(), "failed response was accepted: {result:?}");
}

#[tokio::test]
async fn crlf_is_a_valid_sse_line_ending() {
    let bytes = stream::iter(vec![Ok(Bytes::from_static(
        b"event: ping\r\ndata: {\"type\":\"ping\"}\r\n\r\n",
    ))]);
    let events: Vec<_> = process_message_stream_sse(bytes).collect().await;
    assert_eq!(events.len(), 1, "valid SSE frame silently discarded");
    assert!(matches!(events[0], Ok(MessageStreamEvent::Ping)));
}

#[tokio::test]
async fn incomplete_utf8_at_eof_must_terminate_after_error() {
    let bytes = stream::iter(vec![Ok(Bytes::from_static(&[0xe2, 0x82]))]);
    let events = process_sse(bytes);
    futures::pin_mut!(events);
    assert!(events.next().await.unwrap().is_err());
    assert!(
        events.next().await.is_none(),
        "EOF error repeats indefinitely"
    );
}

#[tokio::test]
async fn oversized_buffer_must_terminate_after_error() {
    let bytes = stream::iter(vec![Ok(Bytes::from(vec![b'x'; 1024 * 1024 + 1]))]);
    let events = process_sse(bytes);
    futures::pin_mut!(events);
    assert!(events.next().await.unwrap().is_err());
    assert!(
        events.next().await.is_none(),
        "buffer limit error repeats indefinitely"
    );
}

#[tokio::test]
async fn slow_consumer_must_still_receive_buffered_events() {
    let bytes = stream::iter(vec![Ok(Bytes::from_static(
        b"event: ping\ndata: {}\n\nevent: ping\ndata: {}\n\n",
    ))]);
    let events = process_sse(bytes);
    futures::pin_mut!(events);
    assert!(events.next().await.unwrap().is_ok());
    assert!(
        events.next().await.unwrap().is_ok(),
        "consumer processing time falsely treated as network inactivity"
    );
}

#[tokio::test]
async fn split_crlf_and_cr_frames_decode() {
    for chunks in [
        vec!["event: ping\r\ndata: {}\r", "\n\r\n"],
        vec!["event: ping\rdata: {}\r\r"],
    ] {
        let events: Vec<_> = process_message_stream_sse(stream::iter(
            chunks.into_iter().map(|s| Ok(Bytes::from(s))),
        ))
        .collect()
        .await;
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], Ok(MessageStreamEvent::Ping)));
    }
}

#[tokio::test]
async fn large_event_and_unknown_event_are_supported() {
    let data = "x".repeat(100_000);
    let body = format!(
        "event: future_event\ndata: {{}}\n\nevent: content_block_start\ndata: {}\n\n",
        json!({"index":0,"content_block":{"type":"web_search_tool_result","tool_use_id":"search","content":[{"type":"web_search_result","encrypted_content":data,"title":"result","url":"https://example.test"}]}})
    );
    let events: Vec<_> = process_message_stream_sse(stream::iter(vec![Ok(Bytes::from(body))]))
        .collect()
        .await;
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        Ok(MessageStreamEvent::ContentBlockStart(_))
    ));
}

#[tokio::test]
async fn malformed_event_terminates_before_later_event() {
    let bytes = stream::iter(vec![Ok(Bytes::from_static(
        b"event: message_start\ndata: invalid\n\nevent: ping\ndata: {}\n\n",
    ))]);
    let events = process_message_stream_sse(bytes);
    futures::pin_mut!(events);
    assert!(events.next().await.unwrap().is_err());
    assert!(events.next().await.is_none());
    assert!(events.next().await.is_none());
}

#[tokio::test]
async fn fallback_message_does_not_make_empty_stream_successful() {
    let fallback = Message::new(
        "fallback".into(),
        vec![],
        Model::Custom("test".into()),
        Usage::new(0, 0),
    );
    let (mut events, message) = AccumulatingStream::new_with_message(stream::empty(), fallback);
    assert!(events.next().await.unwrap().is_err());
    assert!(message.await.unwrap().is_err());
    assert!(events.next().await.is_none());
    assert!(events.next().await.is_none());
}

#[tokio::test]
async fn sparse_block_index_is_rejected_without_allocating() {
    let bad = MessageStreamEvent::ContentBlockStart(ContentBlockStartEvent::new(
        TextBlock::new("bad").into(),
        usize::MAX,
    ));
    let (mut events, message) = AccumulatingStream::new(stream::iter(vec![Ok(start()), Ok(bad)]));
    assert!(events.next().await.unwrap().is_ok());
    assert!(events.next().await.unwrap().is_err());
    assert!(message.await.unwrap().is_err());
    assert!(events.next().await.is_none());
}

#[tokio::test]
async fn stop_reason_is_sufficient_without_message_stop() {
    let terminal = MessageStreamEvent::MessageDelta(MessageDeltaEvent::new(
        MessageDelta::new().with_stop_reason(StopReason::EndTurn),
        MessageDeltaUsage::new(1),
    ));
    let (mut events, message) =
        AccumulatingStream::new(stream::iter(vec![Ok(start()), Ok(terminal)]));
    while let Some(event) = events.next().await {
        event.unwrap();
    }
    assert!(message.await.unwrap().is_ok());
    assert!(events.next().await.is_none());
}
