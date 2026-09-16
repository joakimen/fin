//! Jira Cloud data source.
//!
//! Treats an issue as finished when it was resolved while assigned to the
//! searching user, and reports it against the moment it was resolved.

pub mod auth;
pub mod client;
pub mod map;
pub mod query;

use anyhow::{Context, Result};
use async_trait::async_trait;
use jiff::tz::TimeZone;

use crate::diag::Diag;
use crate::item::{Kind, SourceId};
use crate::period::TimeRange;
use crate::source::{Fetch, Source};

use auth::Site;
use client::JiraApi;

pub fn id() -> SourceId {
    SourceId::new("jira")
}

pub fn default_kinds() -> Vec<Kind> {
    vec![Kind::new("issue")]
}

pub struct Jira {
    api: Box<dyn JiraApi>,
    site: Site,
    email: String,
    projects: Vec<String>,
    tz: TimeZone,
    diag: Diag,
}

impl Jira {
    pub fn new(
        api: Box<dyn JiraApi>,
        site: Site,
        email: String,
        projects: Vec<String>,
        tz: TimeZone,
        diag: Diag,
    ) -> Self {
        Self {
            api,
            site,
            email,
            projects,
            tz,
            diag,
        }
    }
}

#[async_trait]
impl Source for Jira {
    fn id(&self) -> SourceId {
        id()
    }

    fn supported_kinds(&self) -> Vec<Kind> {
        default_kinds()
    }

    fn cache_fingerprint(&self) -> String {
        let mut projects = self.projects.clone();
        projects.sort();
        format!(
            "site={};email={};projects={}",
            self.site,
            self.email,
            projects.join(",")
        )
    }

    async fn fetch(&self, range: &TimeRange, kinds: &[Kind]) -> Result<Fetch> {
        if !kinds.iter().any(|k| k.as_str() == "issue") {
            return Ok(Fetch::default());
        }

        // Jira answers a search with bad credentials as an anonymous user
        // who can see nothing, so the account is confirmed before searching.
        let account = self
            .api
            .myself()
            .await
            .context("could not identify the Jira account")?;
        if account.get("accountId").and_then(|v| v.as_str()).is_none() {
            anyhow::bail!("Jira did not return an account for the credentials");
        }

        let jql = query::resolved_issues(range, &self.projects);
        self.diag.log(format_args!("jira query: {jql}"));

        let mut items = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let body = self
                .api
                .search(query::search_body(&jql, token.as_deref()))
                .await
                .context("Jira search failed")?;
            let page = map::parse_page(&body)?;
            items.extend(map::to_items(&page, &self.site, range, &self.tz));

            match page.next() {
                Some(next) if token.as_deref() != Some(next) => token = Some(next.to_owned()),
                _ => break,
            }
        }

        self.diag
            .log(format_args!("jira issue -> {} items", items.len()));
        Ok(Fetch {
            items,
            warnings: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::period::{self as period, DayName, Period};
    use jiff::Zoned;
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};

    /// Replays canned pages in order, recording the request bodies it receives.
    struct Replay {
        pages: Mutex<Vec<Value>>,
        seen: Arc<Mutex<Vec<Value>>>,
        anonymous: bool,
    }

    impl Replay {
        fn new(pages: Vec<Value>) -> Self {
            Self {
                pages: Mutex::new(pages),
                seen: Arc::new(Mutex::new(Vec::new())),
                anonymous: false,
            }
        }

        fn recorder(&self) -> Arc<Mutex<Vec<Value>>> {
            Arc::clone(&self.seen)
        }
    }

    #[async_trait]
    impl JiraApi for Replay {
        async fn myself(&self) -> Result<Value> {
            if self.anonymous {
                anyhow::bail!("Jira rejected the credentials (401).");
            }
            Ok(json!({ "accountId": "abc123" }))
        }

        async fn search(&self, body: Value) -> Result<Value> {
            self.seen.lock().unwrap().push(body);
            let mut pages = self.pages.lock().unwrap();
            if pages.is_empty() {
                anyhow::bail!("no canned response left");
            }
            Ok(pages.remove(0))
        }
    }

    fn issue(key: &str) -> Value {
        json!({ "key": key, "fields": {
            "summary": "work",
            "resolutiondate": "2026-09-14T11:15:00.000+0200",
            "project": { "key": "ABC", "name": "Alphabet" }
        }})
    }

    fn range() -> TimeRange {
        let now: Zoned = "2026-09-15T14:30:00+02:00[Europe/Oslo]".parse().unwrap();
        period::resolve(&now, Period::Week, DayName::Mon, false).unwrap()
    }

    fn jira(api: Replay, projects: Vec<String>) -> Jira {
        Jira::new(
            Box::new(api),
            Site::parse("example.atlassian.net").unwrap(),
            "me@example.com".into(),
            projects,
            TimeZone::get("Europe/Oslo").unwrap(),
            Diag::new(false),
        )
    }

    #[tokio::test]
    async fn pagination_follows_the_page_token_until_the_last_page() {
        let api = Replay::new(vec![
            json!({ "issues": [issue("ABC-1")], "nextPageToken": "t1", "isLast": false }),
            json!({ "issues": [issue("ABC-2")], "isLast": true }),
        ]);
        let requests = api.recorder();
        let items = jira(api, vec![])
            .fetch(&range(), &default_kinds())
            .await
            .unwrap()
            .items;

        assert_eq!(items.len(), 2);
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].get("nextPageToken").is_none());
        assert_eq!(requests[1]["nextPageToken"], "t1");
    }

    #[tokio::test]
    async fn a_repeated_page_token_ends_pagination() {
        let api = Replay::new(vec![
            json!({ "issues": [issue("ABC-1")], "nextPageToken": "same" }),
            json!({ "issues": [issue("ABC-2")], "nextPageToken": "same" }),
        ]);
        let items = jira(api, vec![])
            .fetch(&range(), &default_kinds())
            .await
            .unwrap()
            .items;
        assert_eq!(items.len(), 2);
    }

    #[tokio::test]
    async fn nothing_is_queried_when_issues_are_not_requested() {
        let api = Replay::new(vec![]);
        let requests = api.recorder();
        let items = jira(api, vec![])
            .fetch(&range(), &[Kind::new("pr")])
            .await
            .unwrap()
            .items;
        assert!(items.is_empty());
        assert!(requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn configured_projects_reach_the_query() {
        let api = Replay::new(vec![json!({ "issues": [], "isLast": true })]);
        let requests = api.recorder();
        jira(api, vec!["ABC".into()])
            .fetch(&range(), &default_kinds())
            .await
            .unwrap();
        let requests = requests.lock().unwrap();
        let jql = requests[0]["jql"].as_str().unwrap();
        assert!(jql.contains(r#"project in ("ABC")"#), "{jql}");
    }

    #[tokio::test]
    async fn rejected_credentials_fail_before_any_search_can_return_nothing() {
        let mut api = Replay::new(vec![json!({ "issues": [], "isLast": true })]);
        api.anonymous = true;
        let requests = api.recorder();
        let err = jira(api, vec![])
            .fetch(&range(), &default_kinds())
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("401"), "{err:#}");
        assert!(requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_transport_failure_fails_the_fetch() {
        let result = jira(Replay::new(vec![]), vec![])
            .fetch(&range(), &default_kinds())
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn the_fingerprint_tracks_site_account_and_projects_but_not_their_order() {
        let fp = |projects: &[&str]| {
            jira(
                Replay::new(vec![]),
                projects.iter().map(|p| p.to_string()).collect(),
            )
            .cache_fingerprint()
        };
        assert_eq!(fp(&["A", "B"]), fp(&["B", "A"]));
        assert_ne!(fp(&[]), fp(&["A"]));

        let other_account = Jira::new(
            Box::new(Replay::new(vec![])),
            Site::parse("example.atlassian.net").unwrap(),
            "someone@example.com".into(),
            vec![],
            TimeZone::UTC,
            Diag::new(false),
        );
        assert_ne!(fp(&[]), other_account.cache_fingerprint());
    }
}
