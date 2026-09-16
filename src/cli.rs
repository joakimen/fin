//! Command-line surface.
//!
//! Parsing only: every flag lands in [`Cli`] unresolved, and precedence
//! against the configuration file is applied in `config::resolve`.

use clap::{ArgAction, Parser};
use jiff::civil::Date;

use crate::item::Kind;
use crate::period::Period;

#[derive(Parser, Debug, Default)]
#[command(
    name = "fin",
    version,
    about = "Report the work you finished, from GitHub, Jira and other sources",
    long_about = None,
    disable_version_flag = true
)]
pub struct Cli {
    /// Print version and exit
    #[arg(short = 'v', long = "version", short_alias = 'V', action = ArgAction::Version)]
    pub version: Option<bool>,

    /// Report on today, from midnight up to now
    #[arg(short = 'd', long, conflicts_with_all = ["week", "month", "since"])]
    pub day: bool,

    /// Report on the current week, from its first day up to now
    #[arg(short = 'w', long, conflicts_with_all = ["day", "month", "since"])]
    pub week: bool,

    /// Report on the current month, from the 1st up to now
    #[arg(short = 'm', long, conflicts_with_all = ["day", "week", "since"])]
    pub month: bool,

    /// Shift the window one whole period back
    #[arg(short = 'p', long, conflicts_with = "since")]
    pub prev: bool,

    /// Start of an explicit window (YYYY-MM-DD)
    #[arg(long, value_name = "DATE")]
    pub since: Option<Date>,

    /// End of an explicit window, inclusive (YYYY-MM-DD); defaults to now
    #[arg(long, value_name = "DATE", requires = "since")]
    pub until: Option<Date>,

    /// Query GitHub
    #[arg(long)]
    pub github: bool,

    /// Query Jira
    #[arg(long)]
    pub jira: bool,

    /// Restrict to an item kind, such as pr or issue; repeatable
    #[arg(long = "type", value_name = "KIND")]
    pub types: Vec<Kind>,

    /// Restrict GitHub results to an organization; repeatable
    #[arg(long = "org", value_name = "ORG")]
    pub orgs: Vec<String>,

    /// Print a markdown table with inline links, without styling
    #[arg(long, conflicts_with = "json")]
    pub markdown: bool,

    /// Print JSON, one object per item
    #[arg(long, conflicts_with = "markdown")]
    pub json: bool,

    /// Ignore cached results and query every source directly
    #[arg(long)]
    pub no_cache: bool,

    /// Delete every cached result and exit
    #[arg(long)]
    pub clear_cache: bool,

    /// Print query diagnostics to stderr
    #[arg(long)]
    pub debug: bool,

    /// Never style output
    #[arg(long)]
    pub no_color: bool,

    /// Read configuration from this path instead of the default location
    #[arg(short = 'c', long, value_name = "PATH")]
    pub config: Option<std::path::PathBuf>,
}

impl Cli {
    /// The period named on the command line, if any.
    pub fn period(&self) -> Option<Period> {
        match (self.day, self.week, self.month) {
            (true, _, _) => Some(Period::Day),
            (_, true, _) => Some(Period::Week),
            (_, _, true) => Some(Period::Month),
            _ => None,
        }
    }

    /// Sources named on the command line. Empty means "whatever is
    /// configured", which the caller resolves.
    pub fn selected_sources(&self) -> Vec<crate::item::SourceId> {
        let mut selected = Vec::new();
        if self.github {
            selected.push(crate::item::SourceId::new("github"));
        }
        if self.jira {
            selected.push(crate::item::SourceId::new("jira"));
        }
        selected
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn week_and_month_are_mutually_exclusive() {
        assert!(Cli::try_parse_from(["fin", "--week", "--month"]).is_err());
    }

    #[test]
    fn day_excludes_every_other_window() {
        assert!(Cli::try_parse_from(["fin", "--day", "--week"]).is_err());
        assert!(Cli::try_parse_from(["fin", "--day", "--month"]).is_err());
        assert!(Cli::try_parse_from(["fin", "--day", "--since", "2026-09-01"]).is_err());
    }

    #[test]
    fn until_requires_since() {
        assert!(Cli::try_parse_from(["fin", "--until", "2026-09-01"]).is_err());
    }

    #[test]
    fn markdown_and_json_are_mutually_exclusive() {
        assert!(Cli::try_parse_from(["fin", "--markdown", "--json"]).is_err());
    }

    #[test]
    fn repeated_type_and_org_flags_accumulate() {
        let cli =
            Cli::try_parse_from(["fin", "--type", "pr", "--type", "issue", "--org", "a"]).unwrap();
        assert_eq!(cli.types.len(), 2);
        assert_eq!(cli.orgs, vec!["a".to_string()]);
    }

    #[test]
    fn period_reflects_the_named_flag() {
        let cli = Cli::try_parse_from(["fin", "-m"]).unwrap();
        assert_eq!(cli.period(), Some(Period::Month));
        let cli = Cli::try_parse_from(["fin", "-d", "-p"]).unwrap();
        assert_eq!(cli.period(), Some(Period::Day));
        assert_eq!(Cli::try_parse_from(["fin"]).unwrap().period(), None);
    }
}
