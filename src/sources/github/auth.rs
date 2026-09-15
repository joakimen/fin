//! GitHub credential discovery.
//!
//! An explicit `GH_TOKEN`/`GITHUB_TOKEN` wins, so the tool works in CI and in
//! containers without the CLI present. Otherwise the token comes from
//! `gh auth token`, which resolves whatever storage `gh` chose — keychain,
//! config file or environment — without this program needing to know which.
//!
//! [`token_from_env`] and [`parse_token_output`] are pure; [`discover`] is the
//! I/O shell around them.

use std::fmt;
use std::process::Command;

use anyhow::{Context, Result, bail};

pub const HOST: &str = "github.com";

/// A GitHub API token. Never renders its value, so it cannot reach a log or a
/// panic message through a derived `Debug`.
#[derive(Clone)]
pub struct Token(String);

impl Token {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

/// Where a token came from, for `--debug` output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Env(&'static str),
    GhCli,
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Env(name) => write!(f, "${name}"),
            Self::GhCli => f.write_str("gh auth token"),
        }
    }
}

pub struct Credentials {
    pub token: Token,
    pub origin: Origin,
}

/// Picks the first environment variable that carries a token.
pub fn token_from_env(lookup: &dyn Fn(&str) -> Option<String>) -> Option<(String, Origin)> {
    for name in ["GH_TOKEN", "GITHUB_TOKEN"] {
        if let Some(value) = lookup(name).filter(|v| !v.trim().is_empty()) {
            return Some((value.trim().to_owned(), Origin::Env(name)));
        }
    }
    None
}

/// Supplies a token from the `gh` CLI.
pub trait GhTokenSource {
    fn token(&self, host: &str) -> Result<String>;
}

pub struct GhCli;

impl GhTokenSource for GhCli {
    fn token(&self, host: &str) -> Result<String> {
        let output = Command::new("gh")
            .args(["auth", "token", "--hostname", host])
            .output()
            .context(
                "could not run `gh`.\n\
                 Install the GitHub CLI, or set GH_TOKEN in the environment.",
            )?;

        parse_token_output(
            output.status.success(),
            &String::from_utf8_lossy(&output.stdout),
            &String::from_utf8_lossy(&output.stderr),
        )
    }
}

/// Interprets the result of `gh auth token`.
pub fn parse_token_output(success: bool, stdout: &str, stderr: &str) -> Result<String> {
    if !success {
        let detail = stderr.trim();
        let detail = if detail.is_empty() {
            "gh reported no detail"
        } else {
            detail
        };
        bail!("`gh auth token` failed: {detail}\nRun `gh auth login` to authenticate.");
    }
    let token = stdout.trim();
    if token.is_empty() {
        bail!("`gh auth token` returned nothing.\nRun `gh auth login` to authenticate.");
    }
    Ok(token.to_owned())
}

/// Finds a usable token, reporting where it came from.
pub fn discover(
    env: &dyn Fn(&str) -> Option<String>,
    gh: &dyn GhTokenSource,
) -> Result<Credentials> {
    if let Some((token, origin)) = token_from_env(env) {
        return Ok(Credentials {
            token: Token::new(token),
            origin,
        });
    }
    let token = gh.token(HOST)?;
    Ok(Credentials {
        token: Token::new(token),
        origin: Origin::GhCli,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedGh(Result<String, String>);

    impl GhTokenSource for FixedGh {
        fn token(&self, _host: &str) -> Result<String> {
            self.0.clone().map_err(|e| anyhow::anyhow!(e))
        }
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn env_token_takes_precedence_and_names_its_variable() {
        let env = |name: &str| (name == "GITHUB_TOKEN").then(|| "gho_env".to_string());
        let (token, origin) = token_from_env(&env).unwrap();
        assert_eq!(token, "gho_env");
        assert_eq!(origin, Origin::Env("GITHUB_TOKEN"));
    }

    #[test]
    fn gh_token_wins_over_github_token() {
        let env = |name: &str| match name {
            "GH_TOKEN" => Some("first".to_string()),
            "GITHUB_TOKEN" => Some("second".to_string()),
            _ => None,
        };
        assert_eq!(token_from_env(&env).unwrap().0, "first");
    }

    #[test]
    fn blank_env_values_are_ignored() {
        let env = |_: &str| Some("   ".to_string());
        assert!(token_from_env(&env).is_none());
    }

    #[test]
    fn the_cli_is_consulted_only_when_the_environment_is_empty() {
        let gh = FixedGh(Ok("gho_from_cli".to_string()));
        let creds = discover(&no_env, &gh).unwrap();
        assert_eq!(creds.token.expose(), "gho_from_cli");
        assert_eq!(creds.origin, Origin::GhCli);

        let env = |name: &str| (name == "GH_TOKEN").then(|| "gho_env".to_string());
        assert_eq!(discover(&env, &gh).unwrap().origin, Origin::Env("GH_TOKEN"));
    }

    #[test]
    fn token_output_is_trimmed_of_its_trailing_newline() {
        assert_eq!(
            parse_token_output(true, "gho_abc\n", "").unwrap(),
            "gho_abc"
        );
    }

    #[test]
    fn a_failing_gh_surfaces_its_own_message_and_the_remedy() {
        let err = parse_token_output(false, "", "You are not logged into any GitHub hosts")
            .unwrap_err()
            .to_string();
        assert!(err.contains("not logged into"), "lost gh's message: {err}");
        assert!(err.contains("gh auth login"), "no remedy offered: {err}");
    }

    #[test]
    fn an_empty_success_is_still_an_error() {
        assert!(parse_token_output(true, "  \n", "").is_err());
    }

    #[test]
    fn a_failing_gh_without_detail_still_errors_usefully() {
        let err = parse_token_output(false, "", "   ")
            .unwrap_err()
            .to_string();
        assert!(err.contains("gh auth login"));
    }

    #[test]
    fn token_debug_output_hides_the_secret() {
        let rendered = format!("{:?}", Token::new("gho_supersecret"));
        assert_eq!(rendered, "Token(<redacted>)");
        assert!(!rendered.contains("supersecret"));
    }
}
