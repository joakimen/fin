//! Conversion of GitHub search responses into [`Item`]s.
//!
//! Pure: the target time zone is a parameter, so mapping is testable against
//! recorded responses without consulting the system clock or zone database.

use anyhow::{Context, Result};
use jiff::{Timestamp, tz::TimeZone};
use serde::Deserialize;

use crate::item::{Item, Kind, SourceId};

#[derive(Debug, Deserialize)]
pub struct SearchPage {
    #[serde(rename = "issueCount")]
    pub issue_count: u32,
    #[serde(rename = "pageInfo")]
    pub page_info: PageInfo,
    pub nodes: Vec<Node>,
}

#[derive(Debug, Deserialize)]
pub struct PageInfo {
    #[serde(rename = "hasNextPage")]
    pub has_next_page: bool,
    #[serde(rename = "endCursor")]
    pub end_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Repository {
    #[serde(rename = "nameWithOwner")]
    pub name_with_owner: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "__typename")]
pub enum Node {
    PullRequest {
        number: u64,
        title: String,
        url: String,
        #[serde(rename = "mergedAt")]
        merged_at: Option<Timestamp>,
        repository: Repository,
    },
    Issue {
        number: u64,
        title: String,
        url: String,
        #[serde(rename = "closedAt")]
        closed_at: Option<Timestamp>,
        #[serde(rename = "stateReason")]
        state_reason: Option<String>,
        repository: Repository,
    },
    #[serde(other)]
    Unknown,
}

/// Turns one page of search results into items in `tz`.
///
/// Nodes without a completion timestamp are dropped: search can return an item
/// whose state changed between matching and being read back. Issues closed as
/// not planned are dropped unless `include_not_planned` is set, since
/// abandoned work is not finished work.
pub fn to_items(page: &SearchPage, tz: &TimeZone, include_not_planned: bool) -> Vec<Item> {
    let source = SourceId::new("github");
    page.nodes
        .iter()
        .filter_map(|node| match node {
            Node::PullRequest {
                number,
                title,
                url,
                merged_at,
                repository,
            } => Some(Item {
                completed_at: merged_at.as_ref()?.to_zoned(tz.clone()),
                source: source.clone(),
                kind: Kind::new("pr"),
                reference: Some(format!("#{number}")),
                title: title.clone(),
                url: url.clone(),
                context: Some(repository.name_with_owner.clone()),
            }),
            Node::Issue {
                number,
                title,
                url,
                closed_at,
                state_reason,
                repository,
            } => {
                let abandoned = state_reason.as_deref() == Some("NOT_PLANNED");
                if abandoned && !include_not_planned {
                    return None;
                }
                Some(Item {
                    completed_at: closed_at.as_ref()?.to_zoned(tz.clone()),
                    source: source.clone(),
                    kind: Kind::new("issue"),
                    reference: Some(format!("#{number}")),
                    title: title.clone(),
                    url: url.clone(),
                    context: Some(repository.name_with_owner.clone()),
                })
            }
            Node::Unknown => None,
        })
        .collect()
}

/// Reads one `search` page out of a GraphQL response body.
pub fn parse_page(body: &serde_json::Value) -> Result<SearchPage> {
    let search = body
        .get("data")
        .and_then(|d| d.get("search"))
        .context("response contained no search results")?;
    serde_json::from_value(search.clone()).context("unexpected search result shape")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tz() -> TimeZone {
        TimeZone::get("Europe/Oslo").unwrap()
    }

    fn body() -> serde_json::Value {
        serde_json::json!({
          "data": { "search": {
            "issueCount": 4,
            "pageInfo": { "hasNextPage": false, "endCursor": "Y3Vyc29y" },
            "nodes": [
              { "__typename": "PullRequest", "number": 12, "title": "Add retries",
                "url": "https://github.com/o/r/pull/12", "mergedAt": "2026-09-14T09:15:00Z",
                "repository": { "nameWithOwner": "o/r" } },
              { "__typename": "PullRequest", "number": 13, "title": "Never merged",
                "url": "https://github.com/o/r/pull/13", "mergedAt": null,
                "repository": { "nameWithOwner": "o/r" } },
              { "__typename": "Issue", "number": 7, "title": "Flaky test",
                "url": "https://github.com/o/r/issues/7", "closedAt": "2026-09-15T07:00:00Z",
                "stateReason": "COMPLETED", "repository": { "nameWithOwner": "o/r" } },
              { "__typename": "Issue", "number": 8, "title": "Wont do",
                "url": "https://github.com/o/r/issues/8", "closedAt": "2026-09-15T07:30:00Z",
                "stateReason": "NOT_PLANNED", "repository": { "nameWithOwner": "o/r" } }
            ]
          }}
        })
    }

    #[test]
    fn maps_pull_requests_and_issues_into_items() {
        let page = parse_page(&body()).unwrap();
        let items = to_items(&page, &tz(), false);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].kind, Kind::new("pr"));
        assert_eq!(items[0].reference.as_deref(), Some("#12"));
        assert_eq!(items[0].title, "Add retries");
        assert_eq!(items[0].context.as_deref(), Some("o/r"));
        assert_eq!(items[1].kind, Kind::new("issue"));
    }

    #[test]
    fn timestamps_are_converted_into_the_target_zone() {
        let page = parse_page(&body()).unwrap();
        let items = to_items(&page, &tz(), false);
        // 09:15Z is 11:15 in Oslo on a summer date, still the 14th.
        assert_eq!(items[0].date(), "2026-09-14");
        assert_eq!(items[0].completed_at.hour(), 11);
    }

    #[test]
    fn items_without_a_completion_time_are_dropped() {
        let page = parse_page(&body()).unwrap();
        let items = to_items(&page, &tz(), false);
        assert!(!items.iter().any(|i| i.title.contains("Never merged")));
    }

    #[test]
    fn issues_closed_as_not_planned_are_excluded_by_default() {
        let page = parse_page(&body()).unwrap();
        assert!(
            !to_items(&page, &tz(), false)
                .iter()
                .any(|i| i.title.contains("Wont do"))
        );
        assert!(
            to_items(&page, &tz(), true)
                .iter()
                .any(|i| i.title.contains("Wont do"))
        );
    }

    #[test]
    fn unknown_node_types_are_ignored_rather_than_failing() {
        let raw = serde_json::json!({ "data": { "search": {
            "issueCount": 1,
            "pageInfo": { "hasNextPage": false, "endCursor": null },
            "nodes": [ { "__typename": "Discussion", "title": "hello" } ]
        }}});
        let page = parse_page(&raw).unwrap();
        assert!(to_items(&page, &tz(), false).is_empty());
    }

    #[test]
    fn page_info_is_read_for_pagination() {
        let page = parse_page(&body()).unwrap();
        assert!(!page.page_info.has_next_page);
        assert_eq!(page.issue_count, 4);
    }

    #[test]
    fn a_response_without_search_data_is_an_error() {
        let raw = serde_json::json!({ "data": {} });
        assert!(parse_page(&raw).is_err());
    }
}
