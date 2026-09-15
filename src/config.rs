//! Configuration file model and precedence resolution.
//!
//! [`ConfigFile`] mirrors `config.toml` and is all-optional; [`Settings`] is
//! the fully-resolved form the rest of the program runs on, with no choices
//! left to make. `resolve` is pure — the clock and the file contents are
//! arguments — so precedence is testable without touching the filesystem.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use jiff::Zoned;
use serde::Deserialize;

use crate::cli::Cli;
use crate::item::{Kind, SourceId};
use crate::period::{self, DayName, Period, TimeRange};

/// How an issue is matched to you, given that GitHub search cannot express
/// "closed by me".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum IssueMatch {
    Author,
    Assignee,
    #[default]
    Either,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct GithubConfig {
    /// Organizations to restrict results to. Empty means every repository the
    /// token can see.
    #[serde(default)]
    pub orgs: Vec<String>,
    #[serde(default)]
    pub issue_match: IssueMatch,
    /// Count issues closed as not planned. Off by default: abandoned work is
    /// not finished work.
    #[serde(default)]
    pub include_not_planned: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    pub first_day_of_week: Option<DayName>,
    /// How long a fetched result stays reusable, such as `15m`. `0` disables
    /// caching.
    pub cache_ttl: Option<String>,
    pub period: Option<Period>,
    pub sources: Option<Vec<SourceId>>,
    #[serde(default)]
    pub types: BTreeMap<SourceId, Vec<Kind>>,
    #[serde(default)]
    pub github: GithubConfig,
}

impl ConfigFile {
    /// Parses configuration from TOML text.
    pub fn parse(text: &str) -> Result<Self> {
        toml::from_str(text).context("invalid configuration")
    }

    /// Loads configuration, returning defaults when the file is absent.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text).with_context(|| format!("in {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
        }
    }
}

/// Resolves the configuration file path.
///
/// Precedence: explicit flag, then `XDG_CONFIG_HOME`, then `~/.config`.
pub fn config_path(flag: Option<&Path>, xdg: Option<&str>, home: Option<&str>) -> Result<PathBuf> {
    if let Some(path) = flag {
        return Ok(path.to_path_buf());
    }
    let base = match xdg.filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => {
            let home = home.filter(|v| !v.is_empty()).context("HOME is not set")?;
            PathBuf::from(home).join(".config")
        }
    };
    Ok(base.join("fin").join("config.toml"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Terminal,
    Markdown,
    Json,
}

/// Fully-resolved run configuration.
#[derive(Debug, Clone)]
pub struct Settings {
    pub range: TimeRange,
    pub sources: Vec<SourceId>,
    pub types: BTreeMap<SourceId, Vec<Kind>>,
    pub format: Format,
    /// False when exactly one source is in play, since the column would then
    /// repeat a single value on every row.
    pub show_source_column: bool,
    pub github: GithubConfig,
    pub cache_ttl: Duration,
}

impl Settings {
    /// Item kinds to request from a source, falling back to the source's own
    /// defaults when nothing is configured for it.
    pub fn kinds_for(&self, source: &SourceId, defaults: &[Kind]) -> Vec<Kind> {
        self.types
            .get(source)
            .filter(|kinds| !kinds.is_empty())
            .cloned()
            .unwrap_or_else(|| defaults.to_vec())
    }
}

/// Cache lifetime applied when the configuration names none.
pub const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(15 * 60);

/// Parses a cache lifetime such as `30s`, `15m` or `2h`.
///
/// A bare `0` disables caching.
pub fn parse_ttl(text: &str) -> Result<Duration> {
    let text = text.trim();
    if text == "0" {
        return Ok(Duration::ZERO);
    }

    let (digits, unit) = text.split_at(
        text.find(|c: char| !c.is_ascii_digit())
            .unwrap_or(text.len()),
    );
    if digits.is_empty() {
        bail!("invalid cache_ttl `{text}` (expected a number followed by s, m or h)");
    }

    let amount: u64 = digits
        .parse()
        .with_context(|| format!("invalid cache_ttl `{text}`"))?;

    let seconds = match unit.trim() {
        "s" => amount,
        "m" | "" => amount * 60,
        "h" => amount * 3600,
        other => bail!("unknown cache_ttl unit `{other}` (expected s, m or h)"),
    };
    Ok(Duration::from_secs(seconds))
}

fn default_sources() -> Vec<SourceId> {
    vec![SourceId::new("github")]
}

/// Merges command-line arguments over file configuration over built-in
/// defaults, producing the settings the run executes with.
pub fn resolve(cli: &Cli, file: ConfigFile, now: &Zoned) -> Result<Settings> {
    let range = match cli.since {
        Some(since) => period::explicit(now, since, cli.until)?,
        None => {
            let period = cli.period().or(file.period).unwrap_or_default();
            let first_day = file.first_day_of_week.unwrap_or_default();
            period::resolve(now, period, first_day, cli.prev)?
        }
    };

    let selected = cli.selected_sources();
    let sources = if !selected.is_empty() {
        selected
    } else {
        file.sources
            .filter(|s| !s.is_empty())
            .unwrap_or_else(default_sources)
    };

    let mut types = file.types;
    if !cli.types.is_empty() {
        for source in &sources {
            types.insert(source.clone(), cli.types.clone());
        }
    }

    let mut github = file.github;
    if !cli.orgs.is_empty() {
        github.orgs = cli.orgs.clone();
    }

    let format = match (cli.markdown, cli.json) {
        (true, _) => Format::Markdown,
        (_, true) => Format::Json,
        _ => Format::Terminal,
    };

    let cache_ttl = if cli.no_cache {
        Duration::ZERO
    } else {
        match &file.cache_ttl {
            Some(text) => parse_ttl(text)?,
            None => DEFAULT_CACHE_TTL,
        }
    };

    if sources.is_empty() {
        bail!("no sources selected");
    }

    Ok(Settings {
        show_source_column: sources.len() > 1,
        range,
        sources,
        types,
        format,
        github,
        cache_ttl,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn now() -> Zoned {
        "2026-09-15T14:30:00+02:00[Europe/Oslo]".parse().unwrap()
    }

    fn cli(args: &[&str]) -> Cli {
        let mut argv = vec!["fin"];
        argv.extend_from_slice(args);
        Cli::try_parse_from(argv).unwrap()
    }

    #[test]
    fn defaults_to_the_current_week_on_github() {
        let s = resolve(&cli(&[]), ConfigFile::default(), &now()).unwrap();
        assert_eq!(s.sources, vec![SourceId::new("github")]);
        assert_eq!(s.range.start.date().to_string(), "2026-09-14");
        assert_eq!(s.format, Format::Terminal);
    }

    #[test]
    fn config_period_applies_when_no_flag_is_given() {
        let file = ConfigFile::parse("period = \"month\"").unwrap();
        let s = resolve(&cli(&[]), file, &now()).unwrap();
        assert_eq!(s.range.start.date().to_string(), "2026-09-01");
    }

    #[test]
    fn a_period_flag_overrides_the_configured_period() {
        let file = ConfigFile::parse("period = \"month\"").unwrap();
        let s = resolve(&cli(&["--week"]), file, &now()).unwrap();
        assert_eq!(s.range.start.date().to_string(), "2026-09-14");
    }

    #[test]
    fn configured_first_day_of_week_moves_the_window() {
        let file = ConfigFile::parse("first_day_of_week = \"sun\"").unwrap();
        let s = resolve(&cli(&["--week"]), file, &now()).unwrap();
        assert_eq!(s.range.start.date().to_string(), "2026-09-13");
    }

    #[test]
    fn explicit_dates_win_over_any_period() {
        let file = ConfigFile::parse("period = \"month\"").unwrap();
        let s = resolve(
            &cli(&["--since", "2026-08-03", "--until", "2026-08-09"]),
            file,
            &now(),
        )
        .unwrap();
        assert_eq!(s.range.start.date().to_string(), "2026-08-03");
        assert_eq!(s.range.end.date().to_string(), "2026-08-10");
    }

    #[test]
    fn source_column_is_dropped_for_a_single_source() {
        let file = ConfigFile::parse("sources = [\"github\", \"todoist\"]").unwrap();
        assert!(resolve(&cli(&[]), file, &now()).unwrap().show_source_column);
        assert!(
            !resolve(&cli(&["--github"]), ConfigFile::default(), &now())
                .unwrap()
                .show_source_column
        );
    }

    #[test]
    fn a_source_flag_narrows_the_configured_set() {
        let file = ConfigFile::parse("sources = [\"github\", \"todoist\"]").unwrap();
        let s = resolve(&cli(&["--github"]), file, &now()).unwrap();
        assert_eq!(s.sources, vec![SourceId::new("github")]);
    }

    #[test]
    fn type_flags_replace_configured_types_for_selected_sources() {
        let file = ConfigFile::parse("[types]\ngithub = [\"pr\", \"issue\"]").unwrap();
        let s = resolve(&cli(&["--type", "pr"]), file, &now()).unwrap();
        assert_eq!(
            s.kinds_for(&SourceId::new("github"), &[]),
            vec![Kind::new("pr")]
        );
    }

    #[test]
    fn kinds_fall_back_to_source_defaults_when_unconfigured() {
        let s = resolve(&cli(&[]), ConfigFile::default(), &now()).unwrap();
        let defaults = vec![Kind::new("pr"), Kind::new("issue")];
        assert_eq!(s.kinds_for(&SourceId::new("github"), &defaults), defaults);
    }

    #[test]
    fn org_flags_replace_configured_orgs() {
        let file = ConfigFile::parse("[github]\norgs = [\"from-config\"]").unwrap();
        let s = resolve(&cli(&["--org", "from-flag"]), file, &now()).unwrap();
        assert_eq!(s.github.orgs, vec!["from-flag".to_string()]);
    }

    #[test]
    fn unknown_configuration_keys_are_rejected() {
        assert!(ConfigFile::parse("perild = \"week\"").is_err());
    }

    #[test]
    fn a_full_configuration_parses() {
        let file = ConfigFile::parse(
            r#"
            first_day_of_week = "mon"
            period = "week"
            sources = ["github"]

            [types]
            github = ["pr", "issue"]

            [github]
            orgs = []
            issue_match = "either"
            "#,
        )
        .unwrap();
        assert_eq!(file.first_day_of_week, Some(DayName::Mon));
        assert_eq!(file.github.issue_match, IssueMatch::Either);
    }

    #[test]
    fn cache_ttl_defaults_to_fifteen_minutes() {
        let s = resolve(&cli(&[]), ConfigFile::default(), &now()).unwrap();
        assert_eq!(s.cache_ttl, Duration::from_secs(900));
    }

    #[test]
    fn configured_cache_ttl_is_honoured() {
        let file = ConfigFile::parse("cache_ttl = \"2h\"").unwrap();
        assert_eq!(resolve(&cli(&[]), file, &now()).unwrap().cache_ttl, Duration::from_secs(7200));
    }

    #[test]
    fn no_cache_flag_overrides_any_configured_ttl() {
        let file = ConfigFile::parse("cache_ttl = \"2h\"").unwrap();
        assert_eq!(resolve(&cli(&["--no-cache"]), file, &now()).unwrap().cache_ttl, Duration::ZERO);
    }

    #[test]
    fn ttl_units_parse() {
        assert_eq!(parse_ttl("45s").unwrap(), Duration::from_secs(45));
        assert_eq!(parse_ttl("15m").unwrap(), Duration::from_secs(900));
        assert_eq!(parse_ttl("2h").unwrap(), Duration::from_secs(7200));
        assert_eq!(parse_ttl(" 15m ").unwrap(), Duration::from_secs(900));
    }

    #[test]
    fn a_bare_number_is_read_as_minutes() {
        assert_eq!(parse_ttl("15").unwrap(), Duration::from_secs(900));
    }

    #[test]
    fn zero_ttl_disables_caching() {
        assert_eq!(parse_ttl("0").unwrap(), Duration::ZERO);
    }

    #[test]
    fn a_malformed_ttl_is_rejected_with_the_accepted_units() {
        let err = parse_ttl("soon").unwrap_err().to_string();
        assert!(err.contains("s, m or h"), "unhelpful error: {err}");
        assert!(parse_ttl("10d").is_err());
        assert!(parse_ttl("").is_err());
    }

    #[test]
    fn config_path_follows_flag_then_xdg_then_home() {
        let flag = PathBuf::from("/tmp/a.toml");
        assert_eq!(
            config_path(Some(&flag), Some("/xdg"), Some("/home")).unwrap(),
            flag
        );
        assert_eq!(
            config_path(None, Some("/xdg"), Some("/home")).unwrap(),
            PathBuf::from("/xdg/fin/config.toml")
        );
        assert_eq!(
            config_path(None, None, Some("/home")).unwrap(),
            PathBuf::from("/home/.config/fin/config.toml")
        );
        assert!(config_path(None, None, None).is_err());
    }
}
