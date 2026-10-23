//! Per-conversation sandbox limits and the timeouts around them.

use std::time::Duration;

// The pool's backstops kill a worker this long past a sandbox time limit at the latest.
pub(super) const LIMIT_MARGIN: Duration = Duration::from_secs(5);
// The `request_timeout` `pool()` sets: the limit on one feed or resume round trip.
pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Limits on one conversation's sandbox.
#[derive(Debug, Clone)]
pub struct Limits {
    /// Bytes of memory the sandbox may use. Default 64 MiB.
    pub max_memory: usize,
    /// Sandbox execution time per snippet, time waiting on the host excluded. Default 10 s.
    pub max_feed_duration: Duration,
    /// Sandbox execution time between two host round trips. Default 60 s.
    pub max_turn_duration: Duration,
    /// Host calls for the whole conversation, across session restarts. Default 50.
    pub max_host_calls: usize,
    /// Characters of one tool result: print output gets this minus 5,000, the final value or
    /// traceback 5,000. At least 5,000. Default 20,000.
    pub max_output_chars: usize,
    /// Time one host function call may take before it raises `RuntimeError`. Default 45 s.
    /// `max_feed_duration` excludes time waiting on the host, so this bounds the host side.
    pub host_call_timeout: Duration,
    /// Time one `python` call may take, host calls included, before it is stopped and the
    /// session discarded. At least the shorter of `max_feed_duration` and `max_turn_duration`
    /// plus 5 s. Default 120 s.
    pub python_call_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_memory: 64 * 1024 * 1024,
            max_feed_duration: Duration::from_secs(10),
            max_turn_duration: Duration::from_secs(60),
            max_host_calls: 50,
            max_output_chars: 20_000,
            host_call_timeout: Duration::from_secs(45),
            python_call_timeout: Duration::from_secs(120),
        }
    }
}
