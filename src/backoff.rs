//! Bounded exponential backoff with an explicit minimum and positive jitter.
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::time::Duration;

/// Returns the delay before retry number `attempt` (0-based) for a given `jitter`.
///
/// The floor is `0.5 s * 2^attempt`, capped at 8 s (reached at attempt 4). The floor is then
/// scaled by `1 + 0.25 * jitter`, with `jitter` clamped to `0.0..=1.0`, and capped at 8 s again,
/// so a delay is never below its floor and never above 8 s.
pub(crate) fn delay(attempt: usize, jitter: f64) -> Duration {
    let floor = (0.5 * 2.0_f64.powi(attempt.min(4) as i32)).min(8.0);
    Duration::from_secs_f64((floor * (1.0 + 0.25 * jitter.clamp(0.0, 1.0))).min(8.0))
}

/// Returns [`delay`] for `attempt` with a jitter drawn from a randomly keyed hasher.
pub(crate) fn next(attempt: usize) -> Duration {
    let random = RandomState::new().hash_one(attempt);
    delay(attempt, random as f64 / u64::MAX as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backoff_floor_and_cap_without_sleep() {
        for attempt in 0..20 {
            let floor =
                Duration::from_secs_f64((0.5 * 2.0_f64.powi(attempt.min(4) as i32)).min(8.0));
            assert_eq!(delay(attempt, 0.0), floor);
            assert!(delay(attempt, 1.0) >= floor);
            assert!(delay(attempt, 1.0) <= Duration::from_secs(8));
        }
    }
}
