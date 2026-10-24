//! Tracing events that replace counters and timing metrics.
//!
//! Every event below is a plain `tracing` event; aggregate them with any subscriber or layer
//! (for example, filter on target and sum the fields).  Durations are in seconds.
//!
//! | Target | Level | Message | Fields |
//! |---|---|---|---|
//! | `carpa::client` | `DEBUG` | `request_completed` | `duration_seconds: f64`, `ok: bool` |
//! | `carpa::client` | `WARN` | `retry` | `error` (Display), `attempt: usize` (1 for the first retry), `backoff_seconds: f64` |
//! | `carpa::stream` | `TRACE` | `event` | (none) |
//! | `carpa::stream` | `TRACE` | `bytes` | `bytes: u64` |
//! | `carpa::stream` | `DEBUG` | `first_byte` | `ttfb_seconds: f64` |
//! | `carpa::stream` | `DEBUG` | `completed` | `duration_seconds: f64` |
//! | `carpa::stream` | `DEBUG` | `error` | (none) |
//! | `carpa::usage` | `INFO` | `usage` | `model`, `input_tokens: u64`, `output_tokens: u64`, `cache_read_input_tokens: u64`, `cache_creation_input_tokens: u64`, `cost_micro_cents: Option<u64>` (debug-formatted; `None` for unknown models) |
//!
//! `carpa::client` `request_completed` fires once per API call (after any retries).  The
//! `carpa::usage` event fires once per completed `send` call and once per stream that
//! reaches `message_stop`.

use std::time::Duration;

/// One API call finished after `duration`; `ok` says whether it succeeded.
pub(crate) fn request_completed(duration: Duration, ok: bool) {
    tracing::debug!(
        target: "carpa::client",
        duration_seconds = duration.as_secs_f64(),
        ok,
        "request_completed"
    );
}

/// One SSE event was parsed.
pub(crate) fn stream_event() {
    tracing::trace!(target: "carpa::stream", "event");
}

/// One chunk of `bytes` bytes arrived on a stream.
pub(crate) fn stream_bytes(bytes: u64) {
    tracing::trace!(target: "carpa::stream", bytes, "bytes");
}

/// The first byte of a stream arrived `time_to_first_byte` after the stream started.
pub(crate) fn stream_first_byte(time_to_first_byte: Duration) {
    tracing::debug!(
        target: "carpa::stream",
        ttfb_seconds = time_to_first_byte.as_secs_f64(),
        "first_byte"
    );
}

/// A stream reached end of input after `duration`.
pub(crate) fn stream_completed(duration: Duration) {
    tracing::debug!(
        target: "carpa::stream",
        duration_seconds = duration.as_secs_f64(),
        "completed"
    );
}

/// A stream failed.
pub(crate) fn stream_error() {
    tracing::debug!(target: "carpa::stream", "error");
}
