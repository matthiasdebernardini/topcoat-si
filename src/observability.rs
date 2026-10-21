//! Tracing events that replace counters and timing metrics.
//!
//! Every event below is a plain `tracing` event; aggregate them with any subscriber or layer
//! (for example, filter on target and sum the fields).  Durations are in seconds.
//!
//! | Target | Level | Message | Fields |
//! |---|---|---|---|
//! | `topcoat_si::client` | `DEBUG` | `request_completed` | `duration_seconds: f64`, `ok: bool` |
//! | `topcoat_si::client` | `DEBUG` | `retry` | `backoff_seconds: f64` |
//! | `topcoat_si::stream` | `TRACE` | `event` | (none) |
//! | `topcoat_si::stream` | `TRACE` | `bytes` | `bytes: u64` |
//! | `topcoat_si::stream` | `DEBUG` | `first_byte` | `ttfb_seconds: f64` |
//! | `topcoat_si::stream` | `DEBUG` | `completed` | `duration_seconds: f64` |
//! | `topcoat_si::stream` | `DEBUG` | `error` | (none) |
//! | `topcoat_si::usage` | `INFO` | `usage` | `model`, `input_tokens: u64`, `output_tokens: u64`, `cache_read_input_tokens: u64`, `cache_creation_input_tokens: u64`, `cost_micro_cents: Option<u64>` (debug-formatted; `None` for unknown models) |
//!
//! `topcoat_si::client` `request_completed` fires once per API call (after any retries).  The
//! `topcoat_si::usage` event fires once per completed `send` call and once per stream that
//! reaches `message_stop`.

use std::time::Duration;

/// One API call finished successfully after `duration`.
pub(crate) fn request_succeeded(duration: Duration) {
    tracing::debug!(
        target: "topcoat_si::client",
        duration_seconds = duration.as_secs_f64(),
        ok = true,
        "request_completed"
    );
}

/// One API call failed after `duration`.
pub(crate) fn request_failed(duration: Duration) {
    tracing::debug!(
        target: "topcoat_si::client",
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
        target: "topcoat_si::client",
        backoff_seconds = backoff.as_secs_f64(),
        "retry"
    );
}

/// One SSE event was parsed.
pub(crate) fn stream_event() {
    tracing::trace!(target: "topcoat_si::stream", "event");
}

/// One chunk of `bytes` bytes arrived on a stream.
pub(crate) fn stream_bytes(bytes: u64) {
    tracing::trace!(target: "topcoat_si::stream", bytes, "bytes");
}

/// The first byte of a stream arrived `ttfb` after the stream started.
pub(crate) fn stream_first_byte(ttfb: Duration) {
    tracing::debug!(
        target: "topcoat_si::stream",
        ttfb_seconds = ttfb.as_secs_f64(),
        "first_byte"
    );
}

/// A stream reached end of input after `duration`.
pub(crate) fn stream_completed(duration: Duration) {
    tracing::debug!(
        target: "topcoat_si::stream",
        duration_seconds = duration.as_secs_f64(),
        "completed"
    );
}

/// A stream failed.
pub(crate) fn stream_error() {
    tracing::debug!(target: "topcoat_si::stream", "error");
}
