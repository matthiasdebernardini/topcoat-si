//! Bounded exponential backoff with an explicit minimum and positive jitter, and the one retry
//! loop every client uses.
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::time::Duration;

use tokio::time::{self, Instant};

use crate::error::{Error, ErrorKind, Result};

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

/// When [`retry`] sends a request again.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RetryPolicy {
    /// Retries after the first attempt.
    pub max_retries: usize,
    /// A longer `retry-after` gives up at once.
    pub max_retry_after: Duration,
    /// The total time across attempts and waits; a wait past it gives up.
    pub budget: Option<Duration>,
    /// Whether a [`ErrorKind::Timeout`] is sent again.
    pub resend_timeouts: bool,
    /// Whether a [`ErrorKind::Interrupted`] is sent again; true only for idempotent requests.
    pub resend_interrupted: bool,
}

/// The result of [`retry`] and how many attempts it took.
#[derive(Debug)]
pub(crate) struct Retried<T> {
    pub result: Result<T>,
    pub attempts: usize,
}

/// Runs `operation` until it succeeds or `policy` gives up, returning the last error.
///
/// `operation` receives the 0-based attempt number and, with a budget, the time left in it.
/// Before each wait `on_retry` receives the error, the attempt number and the delay. The delay is
/// [`next`] or the error's `retry_after`, whichever is longer, so a `retry-after: 0` never sends
/// again at once.
pub(crate) async fn retry<T, Operation, Attempt, OnRetry>(
    policy: &RetryPolicy,
    mut operation: Operation,
    mut on_retry: OnRetry,
) -> Retried<T>
where
    Operation: FnMut(usize, Option<Duration>) -> Attempt,
    Attempt: Future<Output = Result<T>>,
    OnRetry: FnMut(&Error, usize, Duration),
{
    let started = Instant::now();
    let mut attempt = 0;
    loop {
        let remaining = policy
            .budget
            .map(|budget| budget.saturating_sub(started.elapsed()));
        let error = match operation(attempt, remaining).await {
            Ok(value) => {
                return Retried {
                    result: Ok(value),
                    attempts: attempt + 1,
                };
            }
            Err(error) => error,
        };
        let resend = match error.kind() {
            ErrorKind::Timeout => policy.resend_timeouts,
            ErrorKind::Interrupted => policy.resend_interrupted,
            _ => error.is_retryable(),
        };
        let retry_after = error.retry_after();
        let delay = next(attempt).max(retry_after.unwrap_or_default());
        let give_up = attempt >= policy.max_retries
            || !resend
            || retry_after.is_some_and(|wait| wait > policy.max_retry_after)
            || policy
                .budget
                .is_some_and(|budget| started.elapsed() + delay >= budget);
        if give_up {
            return Retried {
                result: Err(error),
                attempts: attempt + 1,
            };
        }
        on_retry(&error, attempt, delay);
        time::sleep(delay).await;
        attempt += 1;
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::error::Service;

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

    const POLICY: RetryPolicy = RetryPolicy {
        max_retries: 3,
        max_retry_after: Duration::from_secs(60),
        budget: None,
        resend_timeouts: false,
        resend_interrupted: false,
    };

    fn overloaded(retry_after: Option<Duration>) -> Error {
        Error::api(
            Service::Anthropic,
            Some(529),
            None,
            "Overloaded",
            retry_after,
        )
    }

    /// Runs [`retry`] over `errors` in order, then successes, and records each wait.
    async fn run(policy: RetryPolicy, errors: Vec<Error>) -> (Retried<usize>, Vec<Duration>) {
        let errors = RefCell::new(errors.into_iter());
        let mut waits = Vec::new();
        let retried = retry(
            &policy,
            |attempt, _| {
                let next = errors.borrow_mut().next();
                async move {
                    match next {
                        Some(error) => Err(error),
                        None => Ok(attempt),
                    }
                }
            },
            |_, _, delay| waits.push(delay),
        )
        .await;
        (retried, waits)
    }

    #[tokio::test(start_paused = true)]
    async fn retries_retryable_errors_until_success() {
        let errors = vec![overloaded(None), Error::new(ErrorKind::Connection, "down")];
        let (retried, waits) = run(POLICY, errors).await;
        assert_eq!(retried.result.expect("success"), 2);
        assert_eq!(retried.attempts, 3);
        assert_eq!(waits.len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn stops_after_max_retries_with_the_last_error() {
        let mut errors: Vec<Error> = Vec::new();
        for status in [500, 502, 503, 504, 529] {
            errors.push(Error::api(
                Service::Anthropic,
                Some(status),
                None,
                "down",
                None,
            ));
        }
        let (retried, waits) = run(POLICY, errors).await;
        let error = retried.result.expect_err("gives up");
        assert_eq!(error.status(), Some(504));
        assert_eq!(retried.attempts, 4);
        assert_eq!(waits.len(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn does_not_retry_a_non_retryable_error() {
        let errors = vec![Error::api(Service::Anthropic, Some(401), None, "no", None)];
        let (retried, waits) = run(POLICY, errors).await;
        assert_eq!(retried.result.expect_err("gives up").status(), Some(401));
        assert_eq!(retried.attempts, 1);
        assert!(waits.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn interrupted_is_resent_only_when_allowed() {
        let interrupted = || vec![Error::new(ErrorKind::Interrupted, "cut off")];
        let (retried, _) = run(POLICY, interrupted()).await;
        assert_eq!(
            retried.result.expect_err("gives up").kind(),
            &ErrorKind::Interrupted
        );
        assert_eq!(retried.attempts, 1);
        let policy = RetryPolicy {
            resend_interrupted: true,
            ..POLICY
        };
        let (retried, _) = run(policy, interrupted()).await;
        assert_eq!(retried.result.expect("resent"), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_is_resent_only_when_allowed() {
        let timeout = || vec![Error::new(ErrorKind::Timeout, "slow")];
        let (retried, _) = run(POLICY, timeout()).await;
        assert_eq!(
            retried.result.expect_err("gives up").kind(),
            &ErrorKind::Timeout
        );
        let policy = RetryPolicy {
            resend_timeouts: true,
            ..POLICY
        };
        let (retried, _) = run(policy, timeout()).await;
        assert_eq!(retried.result.expect("resent"), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn waits_retry_after_and_gives_up_past_the_cap() {
        let (retried, waits) = run(POLICY, vec![overloaded(Some(Duration::from_secs(7)))]).await;
        assert!(retried.result.is_ok());
        assert_eq!(waits, vec![Duration::from_secs(7)]);
        let (retried, waits) = run(POLICY, vec![overloaded(Some(Duration::from_secs(61)))]).await;
        assert_eq!(
            retried.result.expect_err("gives up").retry_after(),
            Some(Duration::from_secs(61))
        );
        assert!(waits.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_short_retry_after_still_waits_the_backoff() {
        let errors = vec![
            overloaded(Some(Duration::ZERO)),
            overloaded(Some(Duration::from_millis(1))),
        ];
        let (retried, waits) = run(POLICY, errors).await;
        assert!(retried.result.is_ok());
        assert_eq!(waits.len(), 2);
        assert!(waits[0] >= delay(0, 0.0), "{waits:?}");
        assert!(waits[1] >= delay(1, 0.0), "{waits:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn gives_up_when_the_wait_would_pass_the_budget() {
        let policy = RetryPolicy {
            budget: Some(Duration::from_secs(10)),
            ..POLICY
        };
        let errors = vec![
            overloaded(Some(Duration::from_secs(6))),
            overloaded(Some(Duration::from_secs(5))),
        ];
        let (retried, waits) = run(policy, errors).await;
        assert_eq!(retried.attempts, 2);
        assert_eq!(waits, vec![Duration::from_secs(6)]);
        assert!(retried.result.is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn operation_sees_the_remaining_budget() {
        let policy = RetryPolicy {
            budget: Some(Duration::from_secs(10)),
            ..POLICY
        };
        let seen = RefCell::new(Vec::new());
        let retried = retry(
            &policy,
            |attempt, remaining| {
                seen.borrow_mut().push(remaining);
                async move {
                    if attempt == 0 {
                        Err(overloaded(Some(Duration::from_secs(4))))
                    } else {
                        Ok(())
                    }
                }
            },
            |_, _, _| {},
        )
        .await;
        assert!(retried.result.is_ok());
        assert_eq!(
            seen.into_inner(),
            vec![Some(Duration::from_secs(10)), Some(Duration::from_secs(6))]
        );
    }
}
