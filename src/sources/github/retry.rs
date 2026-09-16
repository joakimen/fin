//! Bounded retries for the GitHub API.
//!
//! [`decide`] is the whole policy and is pure: given what went wrong, which
//! attempt it was and the current time, it says whether to wait and try again.
//! [`Retrying`] is the loop around it, with the clock and the sleep supplied
//! through [`Timer`] so the loop is testable without waiting.

use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use jiff::Timestamp;
use serde_json::Value;

use super::client::GraphQl;
use crate::diag::Diag;

/// Rate-limit signals GitHub attaches to a response.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RateLimit {
    /// `retry-after`, sent with secondary rate limits.
    pub retry_after: Option<Duration>,
    /// `x-ratelimit-remaining`.
    pub remaining: Option<u64>,
    /// `x-ratelimit-reset`, when the primary limit replenishes.
    pub reset: Option<Timestamp>,
}

impl RateLimit {
    /// Reads the rate-limit headers through `header`, ignoring malformed
    /// values.
    pub fn from_headers<'a>(header: impl Fn(&str) -> Option<&'a str>) -> Self {
        let number = |name: &str| header(name).and_then(|v| v.trim().parse::<u64>().ok());
        Self {
            retry_after: number("retry-after").map(Duration::from_secs),
            remaining: number("x-ratelimit-remaining"),
            reset: number("x-ratelimit-reset")
                .and_then(|secs| i64::try_from(secs).ok())
                .and_then(|secs| Timestamp::from_second(secs).ok()),
        }
    }

    /// Whether a 403 is a rate limit rather than a permissions refusal.
    pub fn is_exhausted(&self) -> bool {
        self.retry_after.is_some() || self.remaining == Some(0)
    }
}

/// How a single request failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The connection failed, timed out, or the body could not be read.
    Transport,
    /// A non-success HTTP status.
    Status(u16, RateLimit),
    /// A successful response whose GraphQL errors report a rate limit.
    RateLimited(RateLimit),
    /// Anything a retry cannot fix, such as a malformed query.
    Permanent,
}

/// A failed request, classified for the retry policy.
#[derive(Debug)]
pub struct AttemptError {
    pub failure: Failure,
    pub error: anyhow::Error,
}

impl AttemptError {
    pub fn new(failure: Failure, error: anyhow::Error) -> Self {
        Self { failure, error }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    pub max_attempts: u32,
    /// Delay before the second attempt; each later attempt doubles it.
    pub base_delay: Duration,
    /// Longest wait a rate limit may impose before fin gives up instead.
    pub max_wait: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_secs(1),
            max_wait: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Retry(Duration),
    /// The failure is not transient.
    Permanent,
    /// The failure was transient, but every attempt is spent.
    Exhausted,
    /// The rate limit lifts later than fin is willing to wait.
    RateLimited {
        wait: Option<Duration>,
    },
}

/// Decides what follows a failed `attempt`, counted from 1.
pub fn decide(policy: &Policy, failure: &Failure, attempt: u32, now: Timestamp) -> Decision {
    let limit = match failure {
        Failure::Permanent => return Decision::Permanent,
        Failure::Transport | Failure::Status(502..=504, _) => None,
        Failure::Status(429, limit) | Failure::RateLimited(limit) => Some(limit),
        Failure::Status(403, limit) if limit.is_exhausted() => Some(limit),
        Failure::Status(..) => return Decision::Permanent,
    };

    let backoff = policy
        .base_delay
        .saturating_mul(2u32.saturating_pow(attempt.saturating_sub(1)));

    let wait = match limit {
        None => backoff,
        Some(limit) => match required_wait(limit, now) {
            Some(wait) if wait > policy.max_wait => {
                return Decision::RateLimited { wait: Some(wait) };
            }
            Some(wait) => wait,
            None => backoff,
        },
    };

    if attempt >= policy.max_attempts {
        return match limit {
            Some(limit) => Decision::RateLimited {
                wait: required_wait(limit, now),
            },
            None => Decision::Exhausted,
        };
    }
    Decision::Retry(wait)
}

/// The wait GitHub asks for, from `retry-after` or else the reset time.
fn required_wait(limit: &RateLimit, now: Timestamp) -> Option<Duration> {
    if let Some(after) = limit.retry_after {
        return Some(after);
    }
    let reset = limit.reset.filter(|_| limit.remaining == Some(0))?;
    let seconds = reset.as_second().saturating_sub(now.as_second()).max(0);
    Some(Duration::from_secs(seconds.unsigned_abs()))
}

/// Explains a rate limit fin will not wait out.
pub fn rate_limit_message(wait: Option<Duration>) -> String {
    match wait {
        Some(wait) => {
            let minutes = wait.as_secs().div_ceil(60).max(1);
            let unit = if minutes == 1 { "minute" } else { "minutes" };
            format!("GitHub's rate limit is exhausted and resets in about {minutes} {unit}")
        }
        None => "GitHub's rate limit is exhausted; try again later".to_string(),
    }
}

/// A single GraphQL request, classified on failure.
#[async_trait]
pub trait Exchange: Send + Sync {
    async fn attempt(&self, query: &str, variables: &Value) -> Result<Value, AttemptError>;
}

#[async_trait]
pub trait Timer: Send + Sync {
    fn now(&self) -> Timestamp;
    async fn sleep(&self, duration: Duration);
}

pub struct TokioTimer;

#[async_trait]
impl Timer for TokioTimer {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }

    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}

/// A [`GraphQl`] client that retries transient failures under a [`Policy`].
pub struct Retrying<E, T> {
    exchange: E,
    timer: T,
    policy: Policy,
    diag: Diag,
}

impl<E: Exchange, T: Timer> Retrying<E, T> {
    pub fn new(exchange: E, timer: T, policy: Policy, diag: Diag) -> Self {
        Self {
            exchange,
            timer,
            policy,
            diag,
        }
    }
}

#[async_trait]
impl<E: Exchange, T: Timer> GraphQl for Retrying<E, T> {
    async fn execute(&self, query: &str, variables: Value) -> Result<Value> {
        let mut attempt = 1;
        loop {
            let failed = match self.exchange.attempt(query, &variables).await {
                Ok(body) => return Ok(body),
                Err(failed) => failed,
            };

            match decide(&self.policy, &failed.failure, attempt, self.timer.now()) {
                Decision::Retry(delay) => {
                    self.diag.log(format_args!(
                        "github attempt {attempt}/{} failed: {:#}; retrying in {}ms",
                        self.policy.max_attempts,
                        failed.error,
                        delay.as_millis()
                    ));
                    self.timer.sleep(delay).await;
                    attempt += 1;
                }
                Decision::Permanent => return Err(failed.error),
                Decision::Exhausted => {
                    return Err(failed
                        .error
                        .context(format!("gave up after {attempt} attempts")));
                }
                Decision::RateLimited { wait } => {
                    return Err(failed.error.context(rate_limit_message(wait)));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use std::sync::Mutex;

    fn now() -> Timestamp {
        "2026-09-15T12:00:00Z".parse().unwrap()
    }

    fn policy() -> Policy {
        Policy::default()
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn transient_failures_back_off_exponentially_until_attempts_run_out() {
        let cases = [
            (Failure::Transport, 1, Decision::Retry(secs(1))),
            (Failure::Transport, 2, Decision::Retry(secs(2))),
            (Failure::Transport, 3, Decision::Exhausted),
            (
                Failure::Status(502, RateLimit::default()),
                1,
                Decision::Retry(secs(1)),
            ),
            (
                Failure::Status(503, RateLimit::default()),
                2,
                Decision::Retry(secs(2)),
            ),
            (
                Failure::Status(504, RateLimit::default()),
                3,
                Decision::Exhausted,
            ),
        ];
        for (failure, attempt, expected) in cases {
            assert_eq!(
                decide(&policy(), &failure, attempt, now()),
                expected,
                "{failure:?} on attempt {attempt}"
            );
        }
    }

    #[test]
    fn failures_a_retry_cannot_fix_are_never_retried() {
        let cases = [
            Failure::Permanent,
            Failure::Status(400, RateLimit::default()),
            Failure::Status(401, RateLimit::default()),
            Failure::Status(403, RateLimit::default()),
            Failure::Status(404, RateLimit::default()),
            Failure::Status(500, RateLimit::default()),
        ];
        for failure in cases {
            assert_eq!(
                decide(&policy(), &failure, 1, now()),
                Decision::Permanent,
                "{failure:?}"
            );
        }
    }

    #[test]
    fn a_rate_limit_waits_as_long_as_github_asks_within_the_cap() {
        let after = RateLimit {
            retry_after: Some(secs(5)),
            ..Default::default()
        };
        let reset = RateLimit {
            remaining: Some(0),
            reset: Some("2026-09-15T12:00:20Z".parse().unwrap()),
            ..Default::default()
        };
        let cases = [
            (Failure::Status(429, after), Decision::Retry(secs(5))),
            (Failure::Status(403, after), Decision::Retry(secs(5))),
            (Failure::Status(403, reset), Decision::Retry(secs(20))),
            (Failure::RateLimited(reset), Decision::Retry(secs(20))),
            (
                Failure::Status(429, RateLimit::default()),
                Decision::Retry(secs(1)),
            ),
        ];
        for (failure, expected) in cases {
            assert_eq!(
                decide(&policy(), &failure, 1, now()),
                expected,
                "{failure:?}"
            );
        }
    }

    #[test]
    fn a_rate_limit_beyond_the_cap_fails_at_once_with_the_wait() {
        let limit = RateLimit {
            remaining: Some(0),
            reset: Some("2026-09-15T12:10:00Z".parse().unwrap()),
            ..Default::default()
        };
        assert_eq!(
            decide(&policy(), &Failure::Status(403, limit), 1, now()),
            Decision::RateLimited {
                wait: Some(secs(600))
            }
        );
    }

    #[test]
    fn a_rate_limit_still_in_force_on_the_last_attempt_is_reported_as_one() {
        let limit = RateLimit {
            retry_after: Some(secs(5)),
            ..Default::default()
        };
        assert_eq!(
            decide(&policy(), &Failure::Status(429, limit), 3, now()),
            Decision::RateLimited {
                wait: Some(secs(5))
            }
        );
    }

    #[test]
    fn a_reset_already_past_retries_immediately() {
        let limit = RateLimit {
            remaining: Some(0),
            reset: Some("2026-09-15T11:59:00Z".parse().unwrap()),
            ..Default::default()
        };
        assert_eq!(
            decide(&policy(), &Failure::Status(403, limit), 1, now()),
            Decision::Retry(Duration::ZERO)
        );
    }

    #[test]
    fn headers_parse_into_rate_limit_signals() {
        let headers = |name: &str| match name {
            "retry-after" => Some("7"),
            "x-ratelimit-remaining" => Some("0"),
            "x-ratelimit-reset" => Some("1789473600"),
            _ => None,
        };
        let limit = RateLimit::from_headers(headers);
        assert_eq!(limit.retry_after, Some(secs(7)));
        assert_eq!(limit.remaining, Some(0));
        assert_eq!(limit.reset.unwrap().as_second(), 1_789_473_600);
        assert!(limit.is_exhausted());
    }

    #[test]
    fn malformed_or_absent_headers_signal_nothing() {
        let limit = RateLimit::from_headers(|name| (name == "retry-after").then_some("soon"));
        assert_eq!(limit, RateLimit::default());
        assert!(!limit.is_exhausted());
    }

    #[test]
    fn the_rate_limit_message_rounds_up_to_whole_minutes() {
        assert!(rate_limit_message(Some(secs(600))).contains("about 10 minutes"));
        assert!(rate_limit_message(Some(secs(31))).contains("about 1 minute"));
        assert!(rate_limit_message(None).contains("try again later"));
    }

    /// Replays scripted outcomes and counts how often it was asked.
    struct Script(Mutex<Vec<Result<Value, AttemptError>>>);

    impl Script {
        fn new(outcomes: Vec<Result<Value, AttemptError>>) -> Self {
            Self(Mutex::new(outcomes))
        }

        fn remaining(&self) -> usize {
            self.0.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl Exchange for &Script {
        async fn attempt(&self, _: &str, _: &Value) -> Result<Value, AttemptError> {
            self.0.lock().unwrap().remove(0)
        }
    }

    /// Records requested sleeps instead of performing them.
    #[derive(Default)]
    struct Recorded(Mutex<Vec<Duration>>);

    #[async_trait]
    impl Timer for &Recorded {
        fn now(&self) -> Timestamp {
            now()
        }

        async fn sleep(&self, duration: Duration) {
            self.0.lock().unwrap().push(duration);
        }
    }

    fn failure(failure: Failure, message: &str) -> Result<Value, AttemptError> {
        Err(AttemptError::new(failure, anyhow!(message.to_owned())))
    }

    #[tokio::test]
    async fn a_transient_failure_is_retried_after_the_backoff() {
        let script = Script::new(vec![
            failure(Failure::Status(502, RateLimit::default()), "bad gateway"),
            Ok(serde_json::json!({ "data": {} })),
        ]);
        let timer = Recorded::default();
        let client = Retrying::new(&script, &timer, policy(), Diag::new(false));

        let body = client.execute("q", Value::Null).await.unwrap();

        assert_eq!(body, serde_json::json!({ "data": {} }));
        assert_eq!(*timer.0.lock().unwrap(), vec![secs(1)]);
        assert_eq!(script.remaining(), 0);
    }

    #[tokio::test]
    async fn exhausted_retries_say_how_many_attempts_were_made() {
        let script = Script::new(vec![
            failure(Failure::Transport, "timed out"),
            failure(Failure::Transport, "timed out"),
            failure(Failure::Transport, "timed out"),
        ]);
        let timer = Recorded::default();
        let client = Retrying::new(&script, &timer, policy(), Diag::new(false));

        let err = client.execute("q", Value::Null).await.unwrap_err();

        assert_eq!(err.to_string(), "gave up after 3 attempts");
        assert_eq!(err.root_cause().to_string(), "timed out");
        assert_eq!(*timer.0.lock().unwrap(), vec![secs(1), secs(2)]);
    }

    #[tokio::test]
    async fn a_permanent_failure_returns_its_own_error_without_sleeping() {
        let script = Script::new(vec![failure(
            Failure::Status(401, RateLimit::default()),
            "GitHub rejected the token (401).",
        )]);
        let timer = Recorded::default();
        let client = Retrying::new(&script, &timer, policy(), Diag::new(false));

        let err = client.execute("q", Value::Null).await.unwrap_err();

        assert_eq!(err.to_string(), "GitHub rejected the token (401).");
        assert!(timer.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_long_rate_limit_fails_without_waiting_it_out() {
        let limit = RateLimit {
            retry_after: Some(secs(3600)),
            ..Default::default()
        };
        let script = Script::new(vec![failure(
            Failure::Status(429, limit),
            "GitHub rate limit exceeded (HTTP 429)",
        )]);
        let timer = Recorded::default();
        let client = Retrying::new(&script, &timer, policy(), Diag::new(false));

        let err = client.execute("q", Value::Null).await.unwrap_err();

        assert!(err.to_string().contains("about 60 minutes"), "{err}");
        assert!(timer.0.lock().unwrap().is_empty());
    }
}
