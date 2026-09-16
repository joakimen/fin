//! Jira Cloud connection settings and credential discovery.
//!
//! The site and account email come from the configuration file, falling back
//! to `JIRA_HOST` and `JIRA_API_USER`; the API token comes only from
//! `JIRA_API_TOKEN`, so it never has to be written to disk. Resolution is pure
//! over an environment lookup.

use std::fmt;

use anyhow::{Result, bail};

use crate::config::JiraConfig;

pub const SITE_VAR: &str = "JIRA_HOST";
pub const EMAIL_VAR: &str = "JIRA_API_USER";
pub const TOKEN_VAR: &str = "JIRA_API_TOKEN";

/// A Jira API token. Never renders its value, so it cannot reach a log or a
/// panic message through a derived `Debug`.
#[derive(Clone)]
pub struct Token(String);

impl Token {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

/// The host name of a Jira Cloud site, such as `example.atlassian.net`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site(String);

impl Site {
    /// Accepts a bare host or an `https://` URL, with or without a trailing
    /// slash.
    pub fn parse(raw: &str) -> Result<Self> {
        let trimmed = raw.trim();
        let lower = trimmed.to_ascii_lowercase();
        let host = if lower.starts_with("https://") {
            &trimmed["https://".len()..]
        } else if lower.contains("://") {
            bail!("invalid Jira site `{trimmed}` (expected a host or an https:// URL)");
        } else {
            trimmed
        };
        let host = host.trim_end_matches('/');

        if host.is_empty() || host.contains(['/', ' ', '?', '#', '@']) {
            bail!("invalid Jira site `{trimmed}` (expected a host such as example.atlassian.net)");
        }
        Ok(Self(host.to_ascii_lowercase()))
    }

    pub fn base_url(&self) -> String {
        format!("https://{}", self.0)
    }

    /// Where a person opens an issue in the browser.
    pub fn browse_url(&self, key: &str) -> String {
        format!("https://{}/browse/{key}", self.0)
    }
}

impl fmt::Display for Site {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Everything needed to talk to one Jira Cloud site as one account.
#[derive(Debug, Clone)]
pub struct Connection {
    pub site: Site,
    pub email: String,
    pub token: Token,
}

/// Resolves the connection from configuration, then the environment.
pub fn resolve(config: &JiraConfig, env: &dyn Fn(&str) -> Option<String>) -> Result<Connection> {
    let site = setting(config.site.as_deref(), SITE_VAR, env).ok_or_else(|| {
        anyhow::anyhow!("no Jira site configured.\nSet `site` under [jira], or {SITE_VAR}.")
    })?;
    let email = setting(config.email.as_deref(), EMAIL_VAR, env).ok_or_else(|| {
        anyhow::anyhow!(
            "no Jira account email configured.\nSet `email` under [jira], or {EMAIL_VAR}."
        )
    })?;
    let token = setting(None, TOKEN_VAR, env).ok_or_else(|| {
        anyhow::anyhow!(
            "{TOKEN_VAR} is not set.\n\
             Create an API token at https://id.atlassian.com/manage-profile/security/api-tokens \
             and export it as {TOKEN_VAR}."
        )
    })?;

    Ok(Connection {
        site: Site::parse(&site)?,
        email,
        token: Token(token),
    })
}

fn setting(
    configured: Option<&str>,
    var: &str,
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    configured
        .map(str::to_owned)
        .or_else(|| env(var))
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_env(name: &str) -> Option<String> {
        match name {
            SITE_VAR => Some("env.atlassian.net".into()),
            EMAIL_VAR => Some("env@example.com".into()),
            TOKEN_VAR => Some("env-token\n".into()),
            _ => None,
        }
    }

    #[test]
    fn the_environment_supplies_everything_when_nothing_is_configured() {
        let c = resolve(&JiraConfig::default(), &full_env).unwrap();
        assert_eq!(c.site.to_string(), "env.atlassian.net");
        assert_eq!(c.email, "env@example.com");
        assert_eq!(c.token.expose(), "env-token");
    }

    #[test]
    fn configured_site_and_email_win_over_the_environment() {
        let config = JiraConfig {
            site: Some("https://file.atlassian.net/".into()),
            email: Some("file@example.com".into()),
            ..Default::default()
        };
        let c = resolve(&config, &full_env).unwrap();
        assert_eq!(c.site.to_string(), "file.atlassian.net");
        assert_eq!(c.email, "file@example.com");
    }

    #[test]
    fn a_missing_site_names_both_the_key_and_the_variable() {
        let env = |name: &str| (name != SITE_VAR).then(|| "x".to_string());
        let err = resolve(&JiraConfig::default(), &env)
            .unwrap_err()
            .to_string();
        assert!(err.contains("`site`"), "{err}");
        assert!(err.contains(SITE_VAR), "{err}");
    }

    #[test]
    fn a_missing_email_names_both_the_key_and_the_variable() {
        let env = |name: &str| (name != EMAIL_VAR).then(|| "x".to_string());
        let err = resolve(&JiraConfig::default(), &env)
            .unwrap_err()
            .to_string();
        assert!(err.contains("`email`"), "{err}");
        assert!(err.contains(EMAIL_VAR), "{err}");
    }

    #[test]
    fn the_token_is_read_only_from_the_environment() {
        let env = |name: &str| (name != TOKEN_VAR).then(|| "example.atlassian.net".to_string());
        let err = resolve(&JiraConfig::default(), &env)
            .unwrap_err()
            .to_string();
        assert!(err.contains(TOKEN_VAR), "{err}");
    }

    #[test]
    fn blank_values_count_as_missing() {
        let env = |_: &str| Some("  ".to_string());
        assert!(resolve(&JiraConfig::default(), &env).is_err());
    }

    #[test]
    fn sites_normalize_to_a_lowercase_host() {
        for raw in [
            "example.atlassian.net",
            "Example.Atlassian.net",
            "https://example.atlassian.net",
            "HTTPS://example.atlassian.net/",
            "  example.atlassian.net  ",
        ] {
            assert_eq!(
                Site::parse(raw).unwrap().to_string(),
                "example.atlassian.net",
                "{raw}"
            );
        }
    }

    #[test]
    fn sites_with_another_scheme_or_a_path_are_rejected() {
        for raw in [
            "",
            "http://example.atlassian.net",
            "https://",
            "example.atlassian.net/jira",
            "https://example.atlassian.net/browse/X-1",
        ] {
            assert!(Site::parse(raw).is_err(), "accepted {raw:?}");
        }
    }

    #[test]
    fn a_site_builds_api_and_browse_urls() {
        let site = Site::parse("example.atlassian.net").unwrap();
        assert_eq!(site.base_url(), "https://example.atlassian.net");
        assert_eq!(
            site.browse_url("ABC-12"),
            "https://example.atlassian.net/browse/ABC-12"
        );
    }

    #[test]
    fn token_debug_output_hides_the_secret() {
        let rendered = format!("{:?}", Token("supersecret".into()));
        assert_eq!(rendered, "Token(<redacted>)");
    }
}
