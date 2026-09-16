//! GraphQL transport for the GitHub API.
//!
//! The [`GraphQl`] trait is the seam that keeps the surrounding source logic
//! testable: everything above it deals in JSON values rather than sockets.
//! [`HttpGraphQl`] makes exactly one request per call and classifies how it
//! failed; retrying is layered on by [`super::retry::Retrying`].

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use serde_json::{Value, json};

use super::auth::Token;
use super::retry::{AttemptError, Exchange, Failure, RateLimit};
use crate::diag::Diag;

pub const DEFAULT_ENDPOINT: &str = "https://api.github.com/graphql";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Upper bound on a whole request, including reading the body. Search pages of
/// a hundred results routinely take a few seconds.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[async_trait]
pub trait GraphQl: Send + Sync {
    async fn execute(&self, query: &str, variables: Value) -> Result<Value>;
}

pub struct HttpGraphQl {
    client: reqwest::Client,
    token: Token,
    endpoint: String,
    diag: Diag,
}

impl HttpGraphQl {
    pub fn new(token: Token, endpoint: impl Into<String>, diag: Diag) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("fin/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .context("could not build the HTTP client")?;
        Ok(Self {
            client,
            token,
            endpoint: endpoint.into(),
            diag,
        })
    }
}

#[async_trait]
impl Exchange for HttpGraphQl {
    async fn attempt(&self, query: &str, variables: &Value) -> Result<Value, AttemptError> {
        let transport = |e: reqwest::Error, what: String| {
            AttemptError::new(Failure::Transport, anyhow::Error::from(e).context(what))
        };

        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(self.token.expose())
            .json(&json!({ "query": query, "variables": variables }))
            .send()
            .await
            .map_err(|e| transport(e, format!("request to {} failed", self.endpoint)))?;

        let status = response.status().as_u16();
        let headers = response.headers();
        let limit = RateLimit::from_headers(|name| headers.get(name).and_then(|v| v.to_str().ok()));
        if let Some(remaining) = limit.remaining {
            self.diag
                .log(format_args!("rate limit remaining: {remaining}"));
        }

        let body = response
            .text()
            .await
            .map_err(|e| transport(e, "could not read the response body".to_string()))?;

        if status != 200 {
            let error = if is_rate_limit(status, &limit) {
                anyhow!("GitHub rate limit exceeded (HTTP {status})")
            } else {
                check_status(status, &body).expect_err("a non-200 status is an error")
            };
            return Err(AttemptError::new(Failure::Status(status, limit), error));
        }

        let json: Value = serde_json::from_str(&body)
            .context("response was not valid JSON")
            .map_err(|e| AttemptError::new(Failure::Permanent, e))?;

        check_graphql_errors(&json).map_err(|e| {
            let failure = if is_graphql_rate_limit(&json) {
                Failure::RateLimited(limit)
            } else {
                Failure::Permanent
            };
            AttemptError::new(failure, e)
        })?;
        Ok(json)
    }
}

/// Whether a failed status reports a rate limit rather than a refusal.
fn is_rate_limit(status: u16, limit: &RateLimit) -> bool {
    status == 429 || (status == 403 && limit.is_exhausted())
}

/// Whether a GraphQL error body reports a rate limit.
///
/// The GraphQL API signals an exhausted primary limit with HTTP 200 and an
/// error of type `RATE_LIMITED`.
pub fn is_graphql_rate_limit(body: &Value) -> bool {
    body.get("errors")
        .and_then(Value::as_array)
        .is_some_and(|errors| {
            errors
                .iter()
                .any(|e| e.get("type").and_then(Value::as_str) == Some("RATE_LIMITED"))
        })
}

/// Turns a transport-level failure into a message that says what to do.
pub fn check_status(status: u16, body: &str) -> Result<()> {
    match status {
        200 => Ok(()),
        401 => bail!(
            "GitHub rejected the token (401).\n\
             Run `gh auth login` to refresh it."
        ),
        403 => bail!(
            "GitHub refused the request (403).\n\
             The token may lack the `repo` and `read:org` scopes, \
             or an organization may require SSO authorization.\n\
             Run `gh auth refresh -s repo,read:org` and authorize the organization."
        ),
        other => bail!("GitHub returned HTTP {other}: {}", truncate(body, 300)),
    }
}

/// Fails on a GraphQL `errors` array.
///
/// The API answers with HTTP 200 for query-level failures, so the body is the
/// only place a bad query, a missing scope or a rate-limit trip shows up.
pub fn check_graphql_errors(body: &Value) -> Result<()> {
    let Some(errors) = body.get("errors").and_then(Value::as_array) else {
        return Ok(());
    };
    if errors.is_empty() {
        return Ok(());
    }
    let joined = errors
        .iter()
        .filter_map(|e| e.get("message").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("; ");
    let joined = if joined.is_empty() {
        truncate(&body.to_string(), 300)
    } else {
        joined
    };
    bail!("GitHub GraphQL error: {joined}");
}

fn truncate(text: &str, limit: usize) -> String {
    let trimmed = text.trim();
    match trimmed.char_indices().nth(limit) {
        None => trimmed.to_owned(),
        Some((cut, _)) => format!("{}…", &trimmed[..cut]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_status_passes() {
        assert!(check_status(200, "{}").is_ok());
    }

    #[test]
    fn unauthorized_status_suggests_reauthenticating() {
        let err = check_status(401, "").unwrap_err().to_string();
        assert!(err.contains("gh auth login"));
    }

    #[test]
    fn forbidden_status_mentions_scopes_and_sso() {
        let err = check_status(403, "").unwrap_err().to_string();
        assert!(err.contains("read:org"));
        assert!(err.contains("SSO"));
    }

    #[test]
    fn other_statuses_include_the_body() {
        let err = check_status(502, "bad gateway").unwrap_err().to_string();
        assert!(err.contains("502"));
        assert!(err.contains("bad gateway"));
    }

    #[test]
    fn graphql_errors_are_surfaced_despite_a_200() {
        let body = serde_json::json!({
            "errors": [{ "message": "Field 'bogus' doesn't exist" }, { "message": "second" }]
        });
        let err = check_graphql_errors(&body).unwrap_err().to_string();
        assert!(err.contains("doesn't exist"));
        assert!(err.contains("second"));
    }

    #[test]
    fn graphql_errors_without_messages_fall_back_to_the_body() {
        let body = serde_json::json!({ "errors": [{ "type": "RATE_LIMITED" }] });
        let err = check_graphql_errors(&body).unwrap_err().to_string();
        assert!(err.contains("RATE_LIMITED"), "body missing from: {err}");
    }

    #[test]
    fn truncation_counts_characters_rather_than_bytes() {
        assert_eq!(truncate("  æøå  ", 3), "æøå");
        assert_eq!(truncate("æøåæøå", 3), "æøå…");
    }

    #[test]
    fn an_absent_or_empty_errors_array_is_fine() {
        assert!(check_graphql_errors(&serde_json::json!({ "data": {} })).is_ok());
        assert!(check_graphql_errors(&serde_json::json!({ "errors": [] })).is_ok());
    }

    #[test]
    fn a_rate_limited_graphql_error_is_recognised() {
        let limited = serde_json::json!({ "errors": [{ "type": "RATE_LIMITED", "message": "API rate limit exceeded" }] });
        let other = serde_json::json!({ "errors": [{ "type": "NOT_FOUND", "message": "nope" }] });
        assert!(is_graphql_rate_limit(&limited));
        assert!(!is_graphql_rate_limit(&other));
        assert!(!is_graphql_rate_limit(&serde_json::json!({ "data": {} })));
    }

    #[test]
    fn a_forbidden_status_is_a_rate_limit_only_when_the_headers_say_so() {
        let exhausted = RateLimit {
            remaining: Some(0),
            ..Default::default()
        };
        assert!(is_rate_limit(429, &RateLimit::default()));
        assert!(is_rate_limit(403, &exhausted));
        assert!(!is_rate_limit(403, &RateLimit::default()));
        assert!(!is_rate_limit(401, &exhausted));
    }

    #[test]
    fn long_bodies_are_truncated() {
        let long = "x".repeat(500);
        let err = check_status(500, &long).unwrap_err().to_string();
        assert!(err.contains('…'));
        assert!(err.len() < 400);
    }
}
