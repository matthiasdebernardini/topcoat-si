use biometrics::{Collector, Counter, Emitter, Gauge, Histogram, Moments, Sensor};
use bytes::Bytes;
use claudius::*;
use futures::{StreamExt, stream};
use std::collections::HashMap;

#[derive(Default)]
struct Snapshot(HashMap<String, u64>);
impl Emitter for Snapshot {
    type Error = std::convert::Infallible;
    fn emit_counter(&mut self, counter: &Counter, _: u64) -> std::result::Result<(), Self::Error> {
        self.0.insert(counter.label().into(), counter.read());
        Ok(())
    }
    fn emit_gauge(&mut self, _: &Gauge, _: u64) -> std::result::Result<(), Self::Error> {
        Ok(())
    }
    fn emit_moments(&mut self, _: &Moments, _: u64) -> std::result::Result<(), Self::Error> {
        Ok(())
    }
    fn emit_histogram(&mut self, _: &Histogram, _: u64) -> std::result::Result<(), Self::Error> {
        Ok(())
    }
}
#[tokio::test]
async fn anthropic_stream_error_increments_error_counter() {
    let collector = Collector::new();
    register_biometrics(&collector);
    let mut before = Snapshot::default();
    collector.emit(&mut before, 1).unwrap();
    let bytes = stream::iter(vec![Ok(Bytes::from_static(b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n"))]);
    let events: Vec<_> = process_message_stream_sse(bytes).collect().await;
    assert_eq!(events.len(), 1);
    assert!(events[0].is_err());
    let mut after = Snapshot::default();
    collector.emit(&mut after, 2).unwrap();
    assert_eq!(
        after.0["claudius.stream.errors"] - before.0["claudius.stream.errors"],
        1,
        "Anthropic error event not counted as a stream error"
    );
}

#[tokio::test]
async fn every_sse_failure_is_counted_once() {
    let collector = Collector::new();
    register_biometrics(&collector);
    let cases = [
        vec![0xff],
        vec![0xe2, 0x82],
        vec![b'x'; 1024 * 1024 + 1],
        format!("event: ping\ndata: {}\n\n", "x".repeat(1024 * 1024)).into_bytes(),
        b"event: ping\n\n".to_vec(),
        b"event: message_start\ndata: invalid\n\n".to_vec(),
    ];
    for body in cases {
        let mut before = Snapshot::default();
        collector.emit(&mut before, 1).unwrap();
        let events: Vec<_> = process_message_stream_sse(stream::iter(vec![Ok(Bytes::from(body))]))
            .collect()
            .await;
        assert_eq!(events.len(), 1);
        assert!(events[0].is_err());
        let mut after = Snapshot::default();
        collector.emit(&mut after, 2).unwrap();
        assert_eq!(
            after.0["claudius.stream.errors"] - before.0["claudius.stream.errors"],
            1
        );
    }
}
