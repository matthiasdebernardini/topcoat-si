//! Bounded exponential backoff with an explicit minimum and positive jitter.
use std::{collections::hash_map::RandomState, hash::BuildHasher, time::Duration};

pub struct ExponentialBackoff;
impl ExponentialBackoff {
    pub fn delay(attempt: usize, jitter: f64) -> Duration {
        let floor = (0.5 * 2.0_f64.powi(attempt.min(4) as i32)).min(8.0);
        Duration::from_secs_f64((floor * (1.0 + 0.25 * jitter.clamp(0.0, 1.0))).min(8.0))
    }
    pub fn next(attempt: usize) -> Duration {
        let random = RandomState::new().hash_one(attempt);
        Self::delay(attempt, random as f64 / u64::MAX as f64)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backoff_floor_and_cap_without_sleep() {
        for attempt in 0..20 {
            let floor =
                Duration::from_secs_f64((0.5 * 2.0_f64.powi(attempt.min(4) as i32)).min(8.0));
            assert_eq!(ExponentialBackoff::delay(attempt, 0.0), floor);
            assert!(ExponentialBackoff::delay(attempt, 1.0) >= floor);
            assert!(ExponentialBackoff::delay(attempt, 1.0) <= Duration::from_secs(8));
        }
    }
}
