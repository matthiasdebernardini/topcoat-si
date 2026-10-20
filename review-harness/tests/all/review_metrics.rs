use bytes::Bytes;
use claudius::*;
use futures::{StreamExt, stream};
use std::sync::{Arc, Mutex};
use tracing::field::{Field, Visit};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::{Layer, Registry};

type Events = Arc<Mutex<Vec<(String, String)>>>;

/// Records the target and message of every event.
struct Capture(Events);

struct Message(String);
impl Visit for Message {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

impl<S: tracing::Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        let mut message = Message(String::new());
        event.record(&mut message);
        self.0
            .lock()
            .unwrap()
            .push((event.metadata().target().to_string(), message.0));
    }
}

fn stream_errors(events: &Events) -> usize {
    events
        .lock()
        .unwrap()
        .iter()
        .filter(|(target, message)| target == "claudius::stream" && message == "error")
        .count()
}

#[tokio::test]
async fn anthropic_stream_error_increments_error_counter() {
    let events = Events::default();
    let _guard =
        tracing::subscriber::set_default(Registry::default().with(Capture(events.clone())));
    let bytes = stream::iter(vec![Ok(Bytes::from_static(b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n"))]);
    let results: Vec<_> = process_message_stream_sse(bytes).collect().await;
    assert_eq!(results.len(), 1);
    assert!(results[0].is_err());
    assert_eq!(
        stream_errors(&events),
        1,
        "Anthropic error event not counted as a stream error"
    );
}

#[tokio::test]
async fn every_sse_failure_is_counted_once() {
    let cases = [
        vec![0xff],
        vec![0xe2, 0x82],
        vec![b'x'; 1024 * 1024 + 1],
        format!("event: ping\ndata: {}\n\n", "x".repeat(1024 * 1024)).into_bytes(),
        b"event: ping\n\n".to_vec(),
        b"event: message_start\ndata: invalid\n\n".to_vec(),
    ];
    for body in cases {
        let events = Events::default();
        let _guard =
            tracing::subscriber::set_default(Registry::default().with(Capture(events.clone())));
        let results: Vec<_> = process_message_stream_sse(stream::iter(vec![Ok(Bytes::from(body))]))
            .collect()
            .await;
        assert_eq!(results.len(), 1);
        assert!(results[0].is_err());
        assert_eq!(stream_errors(&events), 1);
    }
}
