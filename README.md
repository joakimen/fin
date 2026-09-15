# fin

[![ci](https://github.com/joakimen/fin/actions/workflows/ci.yml/badge.svg)](https://github.com/joakimen/fin/actions/workflows/ci.yml)
[![rust](https://img.shields.io/badge/dynamic/toml?url=https%3A%2F%2Fraw.githubusercontent.com%2Fjoakimen%2Ffin%2Fmain%2Frust-toolchain.toml&query=%24.toolchain.channel&label=rust&color=%23dea584)](rust-toolchain.toml)
[![license](https://img.shields.io/github/license/joakimen/fin)](LICENSE)

Report the work you finished, from GitHub and other sources.

`fin` answers "what did I get done this week?", in a shape you can paste into a
status report. It reports *completions*, not activity: a pull request counts when
it merged, an issue when it closed.

```
$ fin
Completed work
Mon 14 Sep – Tue 15 Sep 2026 · 3 items

Monday 14 September
  PR     #128  Retry uploads on 5xx      acme/ingest
  Issue  #57   Flaky integration test    acme/ingest

Tuesday 15 September
  PR     #41   Bump the runtime to 1.97  acme/tooling

────────────────────────────────────────────────────
3 items · 1 Issue · 2 PRs · 2 repositories
```

Titles are clickable in terminals that support hyperlinks. `fin` adds no underline of
its own, so your terminal marks links the way you have configured it. Long titles are
truncated to the terminal width.

Styling uses only the sixteen themeable ANSI colours, so it reads correctly on light
and dark backgrounds alike.

## Install

```sh
mise use -g github:joakimen/fin
```

Or from source:

```sh
make install
```

Run `make check` to format-check, lint and test.

## Authentication

`fin` reuses the GitHub CLI's session, so if `gh auth status` works, so does `fin`.
Set `GH_TOKEN` or `GITHUB_TOKEN` to override that. This is useful in CI, where `gh`
may not be installed.

The token needs `repo` and `read:org` to see private and organization work:

```sh
gh auth refresh -s repo,read:org
```

Organizations with SAML enforcement need the token authorized for that organization.

## Usage

```sh
fin                  # the current week so far
fin -w -p            # all of last week
fin -m               # the current month so far
fin --since 2026-08-01 --until 2026-08-31
fin --markdown       # a table to paste into an issue tracker or wiki
fin --json | jq .    # for anything else
```

| Flag | Meaning |
| --- | --- |
| `-w`, `--week` | The current week, from its first day up to now |
| `-m`, `--month` | The current month, from the 1st up to now |
| `-p`, `--prev` | Shift the window one whole period back |
| `--since`, `--until` | An explicit window; `--until` is inclusive |
| `--github` | Query only GitHub |
| `--type <KIND>` | Restrict to `pr` or `issue`; repeatable |
| `--org <ORG>` | Restrict to an organization; repeatable |
| `--markdown` | A markdown table with inline links, unstyled |
| `--json` | JSON, one object per item |
| `--no-cache` | Ignore cached results and query directly |
| `--clear-cache` | Delete every cached result and exit |
| `--debug` | Print timestamped queries and diagnostics to stderr |
| `--no-color` | Never style output |
| `-c`, `--config <PATH>` | Use this configuration file |
| `-v`, `--version` | Print the version |

Both period flags work at any point during the period, and always run up to the
current moment. Naming a single source drops the source column, since it would
repeat one value on every row.

## Configuration

`fin` reads `~/.config/fin/config.toml`, or `$XDG_CONFIG_HOME/fin/config.toml` when
that is set. Every field is optional, and a flag always wins over the file.

```toml
# Which day a reporting week starts on.
first_day_of_week = "mon"

# The window used when no period flag is given.
period = "week"

# Sources queried when no source flag is given.
sources = ["github"]

# How long a fetched result stays reusable. Accepts s, m or h; "0" disables
# caching entirely.
cache_ttl = "15m"

# Item kinds to fetch per source.
[types]
github = ["pr", "issue"]

[github]
# Organizations to restrict results to. Empty means every repository the token
# can see.
orgs = []

# GitHub search cannot express "closed by me", so issues are matched by
# authorship, assignment, or either.
issue_match = "either"

# Count issues closed as not planned.
include_not_planned = false
```

## Caching

Results are cached under `~/.cache/fin`, or `$XDG_CACHE_HOME/fin` when that is set,
for 15 minutes by default. Repeating a report costs nothing and returns instantly,
which matters while you are still editing the text around it.

A cached window is keyed on its start, the sources and kinds involved, and the
settings that shape the query. Changing any of those fetches afresh. A window that
runs up to *now* is deliberately not keyed on its end, so the TTL is what bounds
how stale a report can be. Use `--no-cache` for a guaranteed-live answer.

```sh
fin --clear-cache
```

## Exit status

`0` on success, `1` if a source could not be reached. A source that fails is
reported on stderr and the remaining sources are still printed, so one outage does
not cost you the whole report.

## Limitations

GitHub search returns at most 1000 results per query. `--debug` says when a window
exceeds that; narrow it, or filter with `--org`.
