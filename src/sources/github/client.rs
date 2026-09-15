//! GraphQL transport for the GitHub API.
//!
//! The [`GraphQl`] trait is the seam that keeps the surrounding source logic
//! testable: everything above it deals in JSON values rather than sockets.

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde_json::{Value, json};

use super::auth::Token;
use crate::diag::Diag;

pub const DEFAULT_ENDPOINT: &str = "https://api.github.com/graphql";

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
impl GraphQl for HttpGraphQl {
    async fn execute(&self, query: &str, variables: Value) -> Result<Value> {
        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(self.token.expose())
            .json(&json!({ "query": query, "variables": variables }))
            .send()
            .await
            .with_context(|| format!("request to {} failed", self.endpoint))?;

        let status = response.status();
        if let Some(remaining) = response
            .headers()
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
        {
            self.diag
                .log(format_args!("rate limit remaining: {remaining}"));
        }

        let body = response
            .text()
            .await
            .context("could not read the response body")?;
        check_status(status.as_u16(), &body)?;

        let json: Value = serde_json::from_str(&body).context("response was not valid JSON")?;
        check_graphql_errors(&json)?;
        Ok(json)
    }
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
    fn an_absent_or_empty_errors_array_is_fine() {
        assert!(check_graphql_errors(&serde_json::json!({ "data": {} })).is_ok());
        assert!(check_graphql_errors(&serde_json::json!({ "errors": [] })).is_ok());
    }

    #[test]
    fn long_bodies_are_truncated() {
        let long = "x".repeat(500);
        let err = check_status(500, &long).unwrap_err().to_string();
        assert!(err.contains('…'));
        assert!(err.len() < 400);
    }
}
