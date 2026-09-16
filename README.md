# fin

[![ci](https://github.com/joakimen/fin/actions/workflows/ci.yml/badge.svg)](https://github.com/joakimen/fin/actions/workflows/ci.yml)
[![rust](https://img.shields.io/badge/dynamic/toml?url=https%3A%2F%2Fraw.githubusercontent.com%2Fjoakimen%2Ffin%2Fmain%2Frust-toolchain.toml&query=%24.toolchain.channel&label=rust&color=%23dea584)](rust-toolchain.toml)
[![license](https://img.shields.io/github/license/joakimen/fin)](LICENSE)

Report the work you finished, from GitHub, Jira and other sources.

`fin` answers "what did I get done this week?", in a shape you can paste into a
status report. It reports *completions*, not activity: a pull request counts when
it merged, a GitHub issue when it closed, a Jira issue when it was resolved.

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
and dark backgrounds alike. Output is unstyled when piped, when `NO_COLOR` is set, or
when `TERM` is `dumb`.

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

### GitHub

`fin` reuses the GitHub CLI's session, so if `gh auth status` works, so does `fin`.
Set `GH_TOKEN` or `GITHUB_TOKEN` to override that. This is useful in CI, where `gh`
may not be installed.

The token needs `repo` and `read:org` to see private and organization work:

```sh
gh auth refresh -s repo,read:org
```

Organizations with SAML enforcement need the token authorized for that organization.

### Jira

`fin` talks to Jira Cloud with an [API token][jira-token] and the email of the
account it belongs to. The token is read only from the environment, so it never
has to be written to the configuration file:

```sh
export JIRA_API_TOKEN=...
export JIRA_API_USER=you@example.com      # or `email` under [jira]
export JIRA_HOST=example.atlassian.net    # or `site` under [jira]
```

A configured `site` or `email` wins over its environment variable. Jira answers
a search with invalid credentials as an anonymous user who can see nothing, so
`fin` confirms the account first and fails instead of reporting an empty week.

[jira-token]: https://id.atlassian.com/manage-profile/security/api-tokens

## Usage

```sh
fin                  # the current week so far
fin -d -p            # yesterday, for a standup
fin -w -p            # all of last week
fin -m               # the current month so far
fin --since 2026-08-01 --until 2026-08-31
fin --markdown       # a table to paste into an issue tracker or wiki
fin --json | jq .    # for anything else
fin --github --jira  # both sources, with a source column
```

| Flag | Meaning |
| --- | --- |
| `-d`, `--day` | Today, from midnight up to now |
| `-w`, `--week` | The current week, from its first day up to now |
| `-m`, `--month` | The current month, from the 1st up to now |
| `-p`, `--prev` | Shift the window one whole period back |
| `--since`, `--until` | An explicit window; `--until` is inclusive |
| `--github` | Query GitHub; combine with other source flags |
| `--jira` | Query Jira; combine with other source flags |
| `--type <KIND>` | Restrict to `pr` or `issue`; repeatable |
| `--org <ORG>` | Restrict GitHub to an organization; repeatable |
| `--markdown` | A markdown table with inline links, unstyled |
| `--json` | JSON, one object per item |
| `--no-cache` | Ignore cached results and query directly |
| `--clear-cache` | Delete every cached result and exit |
| `--debug` | Print timestamped queries and diagnostics to stderr |
| `--no-color` | Never style output |
| `-c`, `--config <PATH>` | Use this configuration file |
| `-v`, `--version` | Print the version |

Every period flag works at any point during the period, and always runs up to the
current moment. Source flags replace the configured sources. Naming a single
source drops the source column, since it would repeat one value on every row.

## Configuration

`fin` reads `~/.config/fin/config.toml`, or `$XDG_CONFIG_HOME/fin/config.toml` when
that is set. Every field is optional, and a flag always wins over the file.

```toml
# Which day a reporting week starts on.
first_day_of_week = "mon"

# The window used when no period flag is given: day, week or month.
period = "week"

# Sources queried when no source flag is given: github, jira.
sources = ["github"]

# How long a fetched result stays reusable. Accepts s, m or h; "0" disables
# caching entirely.
cache_ttl = "15m"

# Item kinds to fetch per source.
[types]
github = ["pr", "issue"]
jira = ["issue"]

[github]
# Organizations to restrict results to. Empty means every repository the token
# can see.
orgs = []

# GitHub search cannot express "closed by me", so issues are matched by
# authorship, assignment, or either.
issue_match = "either"

# Count issues closed as not planned.
include_not_planned = false

[jira]
# Jira Cloud site. Falls back to JIRA_HOST.
site = "example.atlassian.net"

# Email of the account the API token belongs to. Falls back to JIRA_API_USER.
email = "you@example.com"

# Project keys to restrict results to. Empty means every project the account
# can browse.
projects = []
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

`0` on success. `1` on an error, such as an invalid configuration or a source that
could not be reached. `2` on invalid arguments.

A source that fails is reported on stderr and the remaining sources are still
printed, so one outage does not cost you the whole report.

Requests time out after 30 seconds. Timeouts, connection failures, `502`–`504`
responses and short rate limits are retried up to three attempts in total, with
backoff. A rate limit that would take longer than 30 seconds to lift fails at once
and says roughly when it resets. `--debug` shows each retry.

## Limitations

GitHub search returns at most 1000 results per query. When a window matches more,
`fin` prints what it received and warns on stderr that the report is incomplete;
narrow the window, or filter with `--org`.

Jira issues count when they were resolved while assigned to you; Jira does not
record who resolved an issue in a searchable form. JQL reads dates in the time
zone of your Jira profile rather than your machine's, so `fin` searches a window
widened by fourteen hours on each side and keeps only the issues resolved inside
the exact window. Only Jira Cloud is supported.
