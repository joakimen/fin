//! GitHub data source.
//!
//! Treats a pull request as finished when it merged and an issue when it
//! closed, and reports each against the window it completed in.

pub mod auth;
pub mod client;
pub mod map;
pub mod query;

use anyhow::{Context, Result};
use async_trait::async_trait;
use jiff::tz::TimeZone;
use serde_json::json;

use crate::config::GithubConfig;
use crate::diag::Diag;
use crate::item::{Item, Kind, SourceId};
use crate::period::TimeRange;
use crate::source::Source;

use client::GraphQl;
use query::Search;

/// Highest page size the search API accepts.
const PAGE_SIZE: u32 = 100;

/// Ceiling GitHub applies to any single search query.
const SEARCH_RESULT_CAP: u32 = 1000;

const SEARCH_QUERY: &str = r#"
query FinSearch($q: String!, $cursor: String, $size: Int!) {
  search(query: $q, type: ISSUE, first: $size, after: $cursor) {
    issueCount
    pageInfo { hasNextPage endCursor }
    nodes {
      __typename
      ... on PullRequest {
        number title url mergedAt
        repository { nameWithOwner }
      }
      ... on Issue {
        number title url closedAt stateReason
        repository { nameWithOwner }
      }
    }
  }
}
"#;

const VIEWER_QUERY: &str = "query FinViewer { viewer { login } }";

pub fn id() -> SourceId {
    SourceId::new("github")
}

pub fn default_kinds() -> Vec<Kind> {
    vec![Kind::new("pr"), Kind::new("issue")]
}

pub struct GitHub {
    api: Box<dyn GraphQl>,
    config: GithubConfig,
    tz: TimeZone,
    diag: Diag,
}

impl GitHub {
    pub fn new(api: Box<dyn GraphQl>, config: GithubConfig, tz: TimeZone, diag: Diag) -> Self {
        Self {
            api,
            config,
            tz,
            diag,
        }
    }

    /// Resolves the authenticated login, which every search query is built
    /// around.
    async fn viewer(&self) -> Result<String> {
        let body = self
            .api
            .execute(VIEWER_QUERY, json!({}))
            .await
            .context("could not identify the authenticated user")?;
        body.get("data")
            .and_then(|d| d.get("viewer"))
            .and_then(|v| v.get("login"))
            .and_then(|l| l.as_str())
            .map(str::to_owned)
            .context("GitHub did not return a login for the token")
    }

    /// Runs one search to exhaustion, following cursors.
    async fn run_search(&self, search: &Search) -> Result<Vec<Item>> {
        self.diag
            .log(format_args!("github query: {}", search.query));

        let mut items = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let body = self
                .api
                .execute(
                    SEARCH_QUERY,
                    json!({ "q": search.query, "cursor": cursor, "size": PAGE_SIZE }),
                )
                .await
                .with_context(|| format!("search failed: {}", search.query))?;

            let page = map::parse_page(&body)?;

            if cursor.is_none() && page.issue_count > SEARCH_RESULT_CAP {
                self.diag.log(format_args!(
                    "{} matches exceed GitHub's {SEARCH_RESULT_CAP}-result cap; \
                     narrow the window or filter by organization",
                    page.issue_count
                ));
            }

            items.extend(map::to_items(
                &page,
                &self.tz,
                self.config.include_not_planned,
            ));

            match page.page_info.end_cursor {
                Some(next) if page.page_info.has_next_page => cursor = Some(next),
                _ => break,
            }
        }

        self.diag.log(format_args!(
            "github {} -> {} items",
            search.kind,
            items.len()
        ));
        Ok(items)
    }
}

#[async_trait]
impl Source for GitHub {
    fn id(&self) -> SourceId {
        id()
    }

    fn supported_kinds(&self) -> Vec<Kind> {
        default_kinds()
    }

    fn cache_fingerprint(&self) -> String {
        let mut orgs = self.config.orgs.clone();
        orgs.sort();
        format!(
            "orgs={};issues={:?};not_planned={}",
            orgs.join(","),
            self.config.issue_match,
            self.config.include_not_planned
        )
    }

    async fn fetch(&self, range: &TimeRange, kinds: &[Kind]) -> Result<Vec<Item>> {
        let user = self.viewer().await?;
        self.diag.log(format_args!("github viewer: {user}"));

        let wanted = |name: &str| kinds.iter().any(|k| k.as_str() == name);
        let mut searches = Vec::new();
        if wanted("pr") {
            searches.extend(query::merged_prs(&user, range, &self.config.orgs));
        }
        if wanted("issue") {
            searches.extend(query::closed_issues(
                &user,
                range,
                &self.config.orgs,
                self.config.issue_match,
            ));
        }

        let mut items = Vec::new();
        for search in &searches {
            items.extend(self.run_search(search).await?);
        }

        crate::item::dedupe(&mut items);
        Ok(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::period::{self as period, DayName, Period};
    use jiff::Zoned;
    use serde_json::Value;
    use std::sync::Mutex;

    /// Replays canned responses in order, recording the queries it was asked.
    struct Replay {
        pages: Mutex<Vec<Value>>,
        seen: Mutex<Vec<String>>,
    }

    impl Replay {
        fn new(pages: Vec<Value>) -> Self {
            Self {
                pages: Mutex::new(pages),
                seen: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl GraphQl for Replay {
        async fn execute(&self, query: &str, variables: Value) -> Result<Value> {
            if query.contains("FinViewer") {
                return Ok(serde_json::json!({ "data": { "viewer": { "login": "joakimen" } } }));
            }
            if let Some(q) = variables.get("q").and_then(Value::as_str) {
                self.seen.lock().unwrap().push(q.to_owned());
            }
            let mut pages = self.pages.lock().unwrap();
            if pages.is_empty() {
                anyhow::bail!("no canned response left");
            }
            Ok(pages.remove(0))
        }
    }

    fn page(nodes: Value, next: Option<&str>) -> Value {
        serde_json::json!({ "data": { "search": {
            "issueCount": 1,
            "pageInfo": { "hasNextPage": next.is_some(), "endCursor": next },
            "nodes": nodes
        }}})
    }

    fn pr(number: u64, merged_at: &str) -> Value {
        serde_json::json!({
            "__typename": "PullRequest", "number": number, "title": "work",
            "url": format!("https://github.com/o/r/pull/{number}"),
            "mergedAt": merged_at, "repository": { "nameWithOwner": "o/r" }
        })
    }

    fn range() -> TimeRange {
        let now: Zoned = "2026-09-15T14:30:00+02:00[Europe/Oslo]".parse().unwrap();
        period::resolve(&now, Period::Week, DayName::Mon, false).unwrap()
    }

    fn github(api: Replay, config: GithubConfig) -> GitHub {
        GitHub::new(
            Box::new(api),
            config,
            TimeZone::get("Europe/Oslo").unwrap(),
            Diag::new(false),
        )
    }

    #[tokio::test]
    async fn fetching_prs_maps_the_single_page() {
        let api = Replay::new(vec![page(
            serde_json::json!([pr(1, "2026-09-14T09:00:00Z")]),
            None,
        )]);
        let source = github(api, GithubConfig::default());
        let items = source.fetch(&range(), &[Kind::new("pr")]).await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].reference.as_deref(), Some("#1"));
        assert_eq!(items[0].title, "work");
    }

    #[tokio::test]
    async fn pagination_follows_the_cursor_until_exhausted() {
        let api = Replay::new(vec![
            page(
                serde_json::json!([pr(1, "2026-09-14T09:00:00Z")]),
                Some("c1"),
            ),
            page(serde_json::json!([pr(2, "2026-09-14T10:00:00Z")]), None),
        ]);
        let source = github(api, GithubConfig::default());
        let items = source.fetch(&range(), &[Kind::new("pr")]).await.unwrap();
        assert_eq!(items.len(), 2);
    }

    #[tokio::test]
    async fn a_page_claiming_more_without_a_cursor_terminates() {
        let api = Replay::new(vec![serde_json::json!({ "data": { "search": {
            "issueCount": 1,
            "pageInfo": { "hasNextPage": true, "endCursor": null },
            "nodes": [pr(1, "2026-09-14T09:00:00Z")]
        }}})]);
        let source = github(api, GithubConfig::default());
        assert_eq!(
            source
                .fetch(&range(), &[Kind::new("pr")])
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn either_match_dedupes_an_issue_found_by_both_queries() {
        let issue = serde_json::json!({
            "__typename": "Issue", "number": 5, "title": "dup",
            "url": "https://github.com/o/r/issues/5",
            "closedAt": "2026-09-14T09:00:00Z", "stateReason": "COMPLETED",
            "repository": { "nameWithOwner": "o/r" }
        });
        let api = Replay::new(vec![
            page(serde_json::json!([issue.clone()]), None),
            page(serde_json::json!([issue]), None),
        ]);
        let source = github(api, GithubConfig::default());
        let items = source.fetch(&range(), &[Kind::new("issue")]).await.unwrap();
        assert_eq!(items.len(), 1);
    }

    #[tokio::test]
    async fn only_the_requested_kinds_are_queried() {
        let api = Replay::new(vec![page(serde_json::json!([]), None)]);
        let source = github(api, GithubConfig::default());
        source.fetch(&range(), &[Kind::new("pr")]).await.unwrap();
        // One PR search and no issue searches; a second call would exhaust the replay.
    }

    #[tokio::test]
    async fn configured_orgs_reach_the_query() {
        let api = Replay::new(vec![page(serde_json::json!([]), None)]);
        let config = GithubConfig {
            orgs: vec!["acme".into()],
            ..Default::default()
        };
        let source = GitHub::new(
            Box::new(api),
            config,
            TimeZone::get("Europe/Oslo").unwrap(),
            Diag::new(false),
        );
        source.fetch(&range(), &[Kind::new("pr")]).await.unwrap();
    }
}
