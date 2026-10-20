//! Tracing events that replace counters and timing metrics.
//!
//! Every event below is a plain `tracing` event; aggregate them with any subscriber or layer
//! (for example, filter on target and sum the fields).  Durations are in seconds.
//!
//! | Target | Level | Message | Fields |
//! |---|---|---|---|
//! | `claudius::client` | `DEBUG` | `request_completed` | `duration_seconds: f64`, `ok: bool` |
//! | `claudius::client` | `DEBUG` | `retry` | `backoff_seconds: f64` |
//! | `claudius::stream` | `TRACE` | `event` | (none) |
//! | `claudius::stream` | `TRACE` | `bytes` | `bytes: u64` |
//! | `claudius::stream` | `DEBUG` | `first_byte` | `ttfb_seconds: f64` |
//! | `claudius::stream` | `DEBUG` | `completed` | `duration_seconds: f64` |
//! | `claudius::stream` | `DEBUG` | `error` | (none) |
//! | `claudius::usage` | `INFO` | `usage` | `model`, `input_tokens: u64`, `output_tokens: u64`, `cache_read_input_tokens: u64`, `cache_creation_input_tokens: u64`, `cost_micro_cents: Option<u64>` (debug-formatted; `None` for unknown models) |
//!
//! `claudius::client` `request_completed` fires once per API call (after any retries).  The
//! `claudius::usage` event fires once per completed `send` call and once per stream that
//! reaches `message_stop`.

use std::time::Duration;

/// One API call finished successfully after `duration`.
pub(crate) fn request_succeeded(duration: Duration) {
    tracing::debug!(
        target: "claudius::client",
        duration_seconds = duration.as_secs_f64(),
        ok = true,
        "request_completed"
    );
}

/// One API call failed after `duration`.
pub(crate) fn request_failed(duration: Duration) {
    tracing::debug!(
        target: "claudius::client",
        duration_seconds = duration.as_secs_f64(),
        ok = false,
        "request_completed"
    );
}

/// One API call finished after `duration`; report it by the outcome in `result`.
pub(crate) fn request_finished<T, E>(duration: Duration, result: &Result<T, E>) {
    match result {
        Ok(_) => request_succeeded(duration),
        Err(_) => request_failed(duration),
    }
}

/// One retry is about to sleep for `backoff`.
pub(crate) fn retry(backoff: Duration) {
    tracing::debug!(
        target: "claudius::client",
        backoff_seconds = backoff.as_secs_f64(),
        "retry"
    );
}

/// One SSE event was parsed.
pub(crate) fn stream_event() {
    tracing::trace!(target: "claudius::stream", "event");
}

/// One chunk of `bytes` bytes arrived on a stream.
pub(crate) fn stream_bytes(bytes: u64) {
    tracing::trace!(target: "claudius::stream", bytes, "bytes");
}

/// The first byte of a stream arrived `ttfb` after the stream started.
pub(crate) fn stream_first_byte(ttfb: Duration) {
    tracing::debug!(
        target: "claudius::stream",
        ttfb_seconds = ttfb.as_secs_f64(),
        "first_byte"
    );
}

/// A stream reached end of input after `duration`.
pub(crate) fn stream_completed(duration: Duration) {
    tracing::debug!(
        target: "claudius::stream",
        duration_seconds = duration.as_secs_f64(),
        "completed"
    );
}

/// A stream failed.
pub(crate) fn stream_error() {
    tracing::debug!(target: "claudius::stream", "error");
}
