//! HTTP transport for the Jira Cloud search API.
//!
//! The [`JiraApi`] trait is the seam that keeps the source logic testable:
//! everything above it deals in JSON values rather than sockets.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde_json::Value;

use super::auth::{Connection, Site, TOKEN_VAR, Token};
use crate::diag::Diag;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[async_trait]
pub trait JiraApi: Send + Sync {
    /// Fetches the account the credentials belong to, from
    /// `GET /rest/api/3/myself`.
    async fn myself(&self) -> Result<Value>;

    /// Runs one `POST /rest/api/3/search/jql` request.
    async fn search(&self, body: Value) -> Result<Value>;
}

pub struct HttpJira {
    client: reqwest::Client,
    site: Site,
    email: String,
    token: Token,
    diag: Diag,
}

impl HttpJira {
    pub fn new(connection: &Connection, diag: Diag) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("fin/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .context("could not build the HTTP client")?;
        Ok(Self {
            client,
            site: connection.site.clone(),
            email: connection.email.clone(),
            token: connection.token.clone(),
            diag,
        })
    }

    async fn send(&self, request: reqwest::RequestBuilder, label: &str) -> Result<Value> {
        let response = request
            .basic_auth(&self.email, Some(self.token.expose()))
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .with_context(|| format!("request to {} failed", self.site.base_url()))?;

        let status = response.status();
        let text = response
            .text()
            .await
            .context("could not read the response body")?;
        self.diag
            .log(format_args!("jira {label} -> HTTP {}", status.as_u16()));
        check_status(status.as_u16(), &text)?;

        serde_json::from_str(&text).context("response was not valid JSON")
    }
}

#[async_trait]
impl JiraApi for HttpJira {
    async fn myself(&self) -> Result<Value> {
        let endpoint = format!("{}/rest/api/3/myself", self.site.base_url());
        self.send(self.client.get(endpoint), "myself").await
    }

    async fn search(&self, body: Value) -> Result<Value> {
        let endpoint = format!("{}/rest/api/3/search/jql", self.site.base_url());
        self.send(self.client.post(endpoint).json(&body), "search")
            .await
    }
}

/// Turns a transport-level failure into a message that says what to do.
pub fn check_status(status: u16, body: &str) -> Result<()> {
    match status {
        200 => Ok(()),
        400 => bail!("Jira rejected the search (400): {}", error_messages(body)),
        401 => bail!(
            "Jira rejected the credentials (401).\n\
             Check {TOKEN_VAR}, and that the email is the account the token belongs to."
        ),
        403 => bail!(
            "Jira refused the request (403).\n\
             The account may lack permission to browse the projects searched."
        ),
        404 => bail!(
            "Jira returned 404 for its REST API.\n\
             Check that the site is a Jira Cloud site."
        ),
        other => bail!("Jira returned HTTP {other}: {}", truncate(body, 300)),
    }
}

/// Extracts Jira's error list, falling back to the raw body.
fn error_messages(body: &str) -> String {
    let Ok(json) = serde_json::from_str::<Value>(body) else {
        return truncate(body, 300);
    };
    let mut messages: Vec<String> = json
        .get("errorMessages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    if let Some(errors) = json.get("errors").and_then(Value::as_object) {
        messages.extend(
            errors
                .iter()
                .filter_map(|(field, m)| m.as_str().map(|m| format!("{field}: {m}"))),
        );
    }
    if messages.is_empty() {
        truncate(body, 300)
    } else {
        messages.join("; ")
    }
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
    fn a_bad_request_surfaces_jiras_own_messages() {
        let body = r#"{"errorMessages":["Field 'resolutiondate' is odd"],"errors":{"jql":"bad"}}"#;
        let err = check_status(400, body).unwrap_err().to_string();
        assert!(err.contains("Field 'resolutiondate' is odd"), "{err}");
        assert!(err.contains("jql: bad"), "{err}");
    }

    #[test]
    fn a_bad_request_without_structured_errors_shows_the_body() {
        let err = check_status(400, "plain failure").unwrap_err().to_string();
        assert!(err.contains("plain failure"), "{err}");
    }

    #[test]
    fn unauthorized_names_the_token_variable() {
        let err = check_status(401, "").unwrap_err().to_string();
        assert!(err.contains(TOKEN_VAR), "{err}");
    }

    #[test]
    fn forbidden_mentions_permissions() {
        let err = check_status(403, "").unwrap_err().to_string();
        assert!(err.contains("permission"), "{err}");
    }

    #[test]
    fn not_found_points_at_the_site() {
        let err = check_status(404, "").unwrap_err().to_string();
        assert!(err.contains("site"), "{err}");
    }

    #[test]
    fn other_statuses_include_a_truncated_body() {
        let err = check_status(502, &"x".repeat(500)).unwrap_err().to_string();
        assert!(err.contains("502"));
        assert!(err.contains('…'));
        assert!(err.len() < 400);
    }
}
