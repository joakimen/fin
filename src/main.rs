//! `fin` — report the work you finished, from GitHub and other sources.
//!
//! This module is the imperative shell: it reads the clock, the environment,
//! the configuration file and the network, and hands plain data to the pure
//! modules that decide and format. Everything testable lives in those.

mod cache;
mod cli;
mod config;
mod diag;
mod item;
mod period;
mod render;
mod source;
mod sources;

use std::io::{IsTerminal, Write};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use jiff::{Zoned, tz::TimeZone};

use cli::Cli;
use config::{ConfigFile, Format, Settings};
use diag::Diag;
use item::SourceId;
use source::Source;

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => return fail(&anyhow::Error::from(e)),
    };

    match runtime.block_on(run(cli)) {
        Ok(code) => code,
        Err(e) => fail(&e),
    }
}

fn fail(error: &anyhow::Error) -> std::process::ExitCode {
    let style = anstyle::Style::new()
        .bold()
        .fg_color(Some(anstyle::AnsiColor::Red.into()));
    let mut stderr = anstream::stderr();
    let _ = writeln!(stderr, "{style}error:{style:#} {error}");
    for cause in error.chain().skip(1) {
        let _ = writeln!(stderr, "  caused by: {cause}");
    }
    std::process::ExitCode::FAILURE
}

async fn run(cli: Cli) -> Result<std::process::ExitCode> {
    let diag = Diag::new(cli.debug);
    let now = Zoned::now();

    let path = config::config_path(
        cli.config.as_deref(),
        std::env::var("XDG_CONFIG_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )?;
    diag.log(format_args!("config: {}", path.display()));

    let file = ConfigFile::load(&path)?;
    let settings = config::resolve(&cli, file, &now)?;
    diag.log(format_args!("window: {}", settings.range.label()));

    let cache_dir = cache::cache_dir(
        std::env::var("XDG_CACHE_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )?;

    if cli.clear_cache {
        let removed = cache::Cache::new(cache_dir, settings.cache_ttl).clear()?;
        let mut stderr = anstream::stderr();
        let _ = writeln!(stderr, "Removed {removed} cached result(s).");
        return Ok(std::process::ExitCode::SUCCESS);
    }
    diag.log(format_args!(
        "cache: {} (ttl {}s)",
        cache_dir.display(),
        settings.cache_ttl.as_secs()
    ));

    let sources = build_sources(&settings, &cache_dir, diag)?;
    let names: Vec<String> = sources.iter().map(|s| s.id().to_string()).collect();

    let spinner = start_spinner(&names, &settings, diag);
    let fetched = source::fetch_all(&sources, &settings.range, &|source| {
        settings.kinds_for(&source.id(), &source.supported_kinds())
    })
    .await;
    if let Some(spinner) = spinner {
        spinner.finish_and_clear();
    }

    let mut items = Vec::new();
    let mut failures = 0;
    for outcome in fetched {
        match outcome.result {
            Ok(fetched) => items.extend(fetched),
            Err(e) => {
                failures += 1;
                warn(&outcome.source, &e);
            }
        }
    }

    item::dedupe(&mut items);
    item::sort(&mut items);

    let layout = render::Layout {
        show_source: settings.show_source_column,
        styled: should_style(&cli, settings.format),
        width: terminal_width(),
    };
    diag.log(format_args!(
        "output: {:?}, styled {}, width {:?}",
        settings.format, layout.styled, layout.width
    ));
    let output = render::render(settings.format, &items, &settings.range, layout);

    let mut stdout = anstream::stdout();
    write!(stdout, "{output}").context("could not write output")?;
    stdout.flush().context("could not flush output")?;

    Ok(if failures > 0 {
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    })
}

fn warn(source: &SourceId, error: &anyhow::Error) {
    let style = anstyle::Style::new().fg_color(Some(anstyle::AnsiColor::Yellow.into()));
    let mut stderr = anstream::stderr();
    let _ = writeln!(
        stderr,
        "{style}warning:{style:#} {source} unavailable: {error}"
    );
    for cause in error.chain().skip(1) {
        let _ = writeln!(stderr, "  caused by: {cause}");
    }
}

fn build_sources(
    settings: &Settings,
    cache_dir: &std::path::Path,
    diag: Diag,
) -> Result<Vec<Box<dyn Source>>> {
    let tz = TimeZone::system();
    let mut built: Vec<Box<dyn Source>> = Vec::new();

    for id in &settings.sources {
        match id.as_str() {
            "github" => {
                let credentials = sources::github::auth::discover(
                    &|name| std::env::var(name).ok(),
                    &sources::github::auth::GhCli,
                )?;
                diag.log(format_args!("github token from {}", credentials.origin));

                let api = sources::github::client::HttpGraphQl::new(
                    credentials.token,
                    sources::github::client::DEFAULT_ENDPOINT,
                    diag,
                )?;
                let source = sources::github::GitHub::new(
                    Box::new(api),
                    settings.github.clone(),
                    tz.clone(),
                    diag,
                );
                built.push(with_cache(Box::new(source), cache_dir, settings, diag));
            }
            other => {
                anyhow::bail!("unknown source `{other}` (known sources: github)");
            }
        }
    }
    Ok(built)
}

/// Wraps a source so recent results are served from disk.
fn with_cache(
    source: Box<dyn Source>,
    cache_dir: &std::path::Path,
    settings: &Settings,
    diag: Diag,
) -> Box<dyn Source> {
    if settings.cache_ttl.is_zero() {
        return source;
    }
    let cache = cache::Cache::new(cache_dir.to_path_buf(), settings.cache_ttl);
    Box::new(cache::Caching::new(source, cache, diag))
}

/// Shows a spinner while sources are queried, unless it would interleave with
/// diagnostics or land somewhere that is not a terminal.
fn start_spinner(
    names: &[String],
    settings: &Settings,
    diag: Diag,
) -> Option<indicatif::ProgressBar> {
    if diag.enabled() || !std::io::stderr().is_terminal() || names.is_empty() {
        return None;
    }
    let style = indicatif::ProgressStyle::with_template("{spinner:.cyan} {msg}").ok()?;
    let spinner = indicatif::ProgressBar::new_spinner()
        .with_style(style)
        .with_message(format!(
            "Querying {} · {}",
            names
                .iter()
                .map(|n| render::source_label(n))
                .collect::<Vec<_>>()
                .join(", "),
            settings.range.label()
        ));
    spinner.enable_steady_tick(Duration::from_millis(80));
    Some(spinner)
}

/// Width available for output, when stdout is a terminal whose size is known.
///
/// Absent when piped, so redirected output keeps full titles. `COLUMNS` is
/// honoured as a fallback for terminals that report no size.
fn terminal_width() -> Option<usize> {
    if !std::io::stdout().is_terminal() {
        return None;
    }
    terminal_size::terminal_size()
        .map(|(terminal_size::Width(w), _)| usize::from(w))
        .filter(|w| *w > 0)
        .or_else(|| {
            std::env::var("COLUMNS")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .filter(|w| *w > 0)
        })
}

/// Decides whether to emit styling.
///
/// Only the terminal format is ever styled; markdown and JSON are meant to be
/// copied or piped verbatim.
fn should_style(cli: &Cli, format: Format) -> bool {
    if format != Format::Terminal || cli.no_color {
        return false;
    }
    if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    if std::env::var("TERM").is_ok_and(|t| t == "dumb") {
        return false;
    }
    std::io::stdout().is_terminal()
}
