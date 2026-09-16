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
use crate::source::{Fetch, Source};

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

    /// Runs one search to exhaustion, following cursors, and reports whether
    /// GitHub's result cap cut it short.
    async fn run_search(&self, search: &Search) -> Result<(Vec<Item>, Option<String>)> {
        self.diag
            .log(format_args!("github query: {}", search.query));

        let mut items = Vec::new();
        let mut cursor: Option<String> = None;
        let mut capped = None;

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

            if cursor.is_none() {
                capped = cap_warning(search.kind, page.issue_count);
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
        Ok((items, capped))
    }
}

/// Describes a search that matched more than GitHub will return, if it did.
fn cap_warning(kind: &str, matched: u32) -> Option<String> {
    let noun = match kind {
        "pr" => "pull request",
        other => other,
    };
    (matched > SEARCH_RESULT_CAP).then(|| {
        format!(
            "the {noun} search matched {matched} results, but GitHub returns at most \
             {SEARCH_RESULT_CAP}, so the report is incomplete; \
             narrow the window or filter with --org"
        )
    })
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

    async fn fetch(&self, range: &TimeRange, kinds: &[Kind]) -> Result<Fetch> {
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

        let mut fetch = Fetch::default();
        for search in &searches {
            let (items, capped) = self.run_search(search).await?;
            fetch.items.extend(items);
            fetch.warnings.extend(capped);
        }

        crate::item::dedupe(&mut fetch.items);
        Ok(fetch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::period::{self as period, DayName, Period};
    use jiff::Zoned;
    use serde_json::Value;
    use std::sync::{Arc, Mutex};

    /// Replays canned responses in order, recording the queries it was asked.
    struct Replay {
        pages: Mutex<Vec<Value>>,
        seen: Arc<Mutex<Vec<String>>>,
    }

    impl Replay {
        fn new(pages: Vec<Value>) -> Self {
            Self {
                pages: Mutex::new(pages),
                seen: Arc::new(Mutex::new(Vec::new())),
            }
        }

        /// Handle on the search queries this double is asked for, readable
        /// after the double has been handed to the source under test.
        fn recorder(&self) -> Arc<Mutex<Vec<String>>> {
            Arc::clone(&self.seen)
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
        counted_page(1, nodes, next)
    }

    fn counted_page(matched: u32, nodes: Value, next: Option<&str>) -> Value {
        serde_json::json!({ "data": { "search": {
            "issueCount": matched,
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
        let items = source
            .fetch(&range(), &[Kind::new("pr")])
            .await
            .unwrap()
            .items;
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
        let items = source
            .fetch(&range(), &[Kind::new("pr")])
            .await
            .unwrap()
            .items;
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
                .items
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
        let items = source
            .fetch(&range(), &[Kind::new("issue")])
            .await
            .unwrap()
            .items;
        assert_eq!(items.len(), 1);
    }

    #[tokio::test]
    async fn a_search_over_the_result_cap_warns_but_still_returns_its_items() {
        let api = Replay::new(vec![counted_page(
            1500,
            serde_json::json!([pr(1, "2026-09-14T09:00:00Z")]),
            None,
        )]);
        let source = github(api, GithubConfig::default());
        let fetch = source.fetch(&range(), &[Kind::new("pr")]).await.unwrap();
        assert_eq!(fetch.items.len(), 1);
        assert_eq!(fetch.warnings.len(), 1);
        assert!(fetch.warnings[0].contains("1500"), "{:?}", fetch.warnings);
        assert!(fetch.warnings[0].contains("--org"), "{:?}", fetch.warnings);
    }

    #[tokio::test]
    async fn a_search_within_the_result_cap_raises_no_warning() {
        let api = Replay::new(vec![counted_page(
            1000,
            serde_json::json!([pr(1, "2026-09-14T09:00:00Z")]),
            None,
        )]);
        let source = github(api, GithubConfig::default());
        let fetch = source.fetch(&range(), &[Kind::new("pr")]).await.unwrap();
        assert!(fetch.warnings.is_empty(), "{:?}", fetch.warnings);
    }

    #[test]
    fn the_cap_warning_names_the_search_it_concerns() {
        assert!(
            cap_warning("pr", 1001)
                .unwrap()
                .contains("pull request search")
        );
        assert!(cap_warning("issue", 1001).unwrap().contains("issue search"));
        assert!(cap_warning("issue", 1000).is_none());
    }

    #[tokio::test]
    async fn only_the_requested_kinds_are_queried() {
        let api = Replay::new(vec![page(serde_json::json!([]), None)]);
        let queries = api.recorder();
        let source = github(api, GithubConfig::default());

        source.fetch(&range(), &[Kind::new("pr")]).await.unwrap();

        let queries = queries.lock().unwrap();
        assert_eq!(queries.len(), 1, "expected exactly one search: {queries:?}");
        assert!(queries[0].contains("is:pr"));
        assert!(!queries.iter().any(|q| q.contains("is:issue")));
    }

    #[tokio::test]
    async fn configured_orgs_reach_the_query() {
        let api = Replay::new(vec![page(serde_json::json!([]), None)]);
        let queries = api.recorder();
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

        let queries = queries.lock().unwrap();
        assert!(
            queries.iter().all(|q| q.contains("org:acme")),
            "organization missing from {queries:?}"
        );
    }

    #[tokio::test]
    async fn the_fingerprint_tracks_every_setting_that_shapes_a_query() {
        let base = github(Replay::new(vec![]), GithubConfig::default()).cache_fingerprint();

        let with_org = github(
            Replay::new(vec![]),
            GithubConfig {
                orgs: vec!["acme".into()],
                ..Default::default()
            },
        )
        .cache_fingerprint();
        let with_match = github(
            Replay::new(vec![]),
            GithubConfig {
                issue_match: crate::config::IssueMatch::Author,
                ..Default::default()
            },
        )
        .cache_fingerprint();
        let with_not_planned = github(
            Replay::new(vec![]),
            GithubConfig {
                include_not_planned: true,
                ..Default::default()
            },
        )
        .cache_fingerprint();

        assert_ne!(base, with_org);
        assert_ne!(base, with_match);
        assert_ne!(base, with_not_planned);
    }

    #[tokio::test]
    async fn the_fingerprint_ignores_the_order_organizations_are_listed_in() {
        let forward = github(
            Replay::new(vec![]),
            GithubConfig {
                orgs: vec!["a".into(), "b".into()],
                ..Default::default()
            },
        )
        .cache_fingerprint();
        let reverse = github(
            Replay::new(vec![]),
            GithubConfig {
                orgs: vec!["b".into(), "a".into()],
                ..Default::default()
            },
        )
        .cache_fingerprint();

        assert_eq!(forward, reverse);
    }
}
