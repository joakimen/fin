//! Conversion of Jira search responses into [`Item`]s.
//!
//! Pure: the site, the window and the target time zone are parameters.

use anyhow::{Context, Result};
use jiff::{Timestamp, tz::TimeZone};
use serde::{Deserialize, Deserializer};

use super::auth::Site;
use crate::item::{Item, Kind, SourceId};
use crate::period::TimeRange;

#[derive(Debug, Deserialize)]
pub struct SearchPage {
    #[serde(default)]
    pub issues: Vec<Issue>,
    #[serde(rename = "nextPageToken")]
    pub next_page_token: Option<String>,
    #[serde(rename = "isLast")]
    pub is_last: Option<bool>,
}

impl SearchPage {
    /// The token for the following page, when there is one.
    pub fn next(&self) -> Option<&str> {
        if self.is_last == Some(true) {
            return None;
        }
        self.next_page_token.as_deref()
    }
}

#[derive(Debug, Deserialize)]
pub struct Issue {
    pub key: String,
    pub fields: Fields,
}

#[derive(Debug, Deserialize)]
pub struct Fields {
    #[serde(default)]
    pub summary: String,
    #[serde(default, deserialize_with = "jira_timestamp")]
    pub resolutiondate: Option<Timestamp>,
    pub project: Option<Project>,
}

#[derive(Debug, Deserialize)]
pub struct Project {
    pub key: String,
    pub name: Option<String>,
}

/// Parses Jira's timestamp form, such as `2026-09-14T11:15:00.000+0200`.
fn jira_timestamp<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Timestamp>, D::Error> {
    let Some(text) = Option::<String>::deserialize(d)? else {
        return Ok(None);
    };
    Timestamp::strptime("%Y-%m-%dT%H:%M:%S%.f%z", &text)
        .or_else(|_| text.parse::<Timestamp>())
        .map(Some)
        .map_err(|e| serde::de::Error::custom(format!("invalid timestamp `{text}`: {e}")))
}

/// Turns one page of search results into items in `tz`.
///
/// Unresolved issues are dropped, as are issues resolved outside `range`: the
/// query window is deliberately wider than the report's.
pub fn to_items(page: &SearchPage, site: &Site, range: &TimeRange, tz: &TimeZone) -> Vec<Item> {
    let source = SourceId::new("jira");
    let (start, end) = (range.start.timestamp(), range.end.timestamp());
    page.issues
        .iter()
        .filter_map(|issue| {
            let resolved = issue.fields.resolutiondate?;
            if resolved < start || resolved >= end {
                return None;
            }
            Some(Item {
                completed_at: resolved.to_zoned(tz.clone()),
                source: source.clone(),
                kind: Kind::new("issue"),
                reference: Some(issue.key.clone()),
                title: issue.fields.summary.clone(),
                url: site.browse_url(&issue.key),
                context: issue
                    .fields
                    .project
                    .as_ref()
                    .map(|p| p.name.clone().unwrap_or_else(|| p.key.clone())),
            })
        })
        .collect()
}

pub fn parse_page(body: &serde_json::Value) -> Result<SearchPage> {
    serde_json::from_value(body.clone()).context("unexpected Jira search result shape")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::period;
    use jiff::Zoned;

    fn tz() -> TimeZone {
        TimeZone::get("Europe/Oslo").unwrap()
    }

    fn site() -> Site {
        Site::parse("example.atlassian.net").unwrap()
    }

    fn range() -> TimeRange {
        let now: Zoned = "2026-09-15T14:30:00+02:00[Europe/Oslo]".parse().unwrap();
        period::explicit(&now, "2026-09-14".parse().unwrap(), None).unwrap()
    }

    fn issue(key: &str, resolved: Option<&str>) -> serde_json::Value {
        serde_json::json!({
            "id": "10001", "key": key,
            "fields": {
                "summary": format!("Work on {key}"),
                "resolutiondate": resolved,
                "project": { "key": "ABC", "name": "Alphabet" }
            }
        })
    }

    fn page(issues: Vec<serde_json::Value>) -> SearchPage {
        parse_page(&serde_json::json!({ "issues": issues, "isLast": true })).unwrap()
    }

    #[test]
    fn maps_a_resolved_issue_into_an_item() {
        let items = to_items(
            &page(vec![issue("ABC-12", Some("2026-09-14T11:15:00.000+0200"))]),
            &site(),
            &range(),
            &tz(),
        );
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.source, SourceId::new("jira"));
        assert_eq!(item.kind, Kind::new("issue"));
        assert_eq!(item.reference.as_deref(), Some("ABC-12"));
        assert_eq!(item.title, "Work on ABC-12");
        assert_eq!(item.url, "https://example.atlassian.net/browse/ABC-12");
        assert_eq!(item.context.as_deref(), Some("Alphabet"));
        assert_eq!(item.completed_at.hour(), 11);
    }

    #[test]
    fn resolution_offsets_are_honoured_when_converting_zones() {
        let items = to_items(
            &page(vec![issue("ABC-1", Some("2026-09-14T05:00:00.000-0400"))]),
            &site(),
            &range(),
            &tz(),
        );
        assert_eq!(items[0].completed_at.hour(), 11);
    }

    #[test]
    fn issues_resolved_outside_the_exact_window_are_dropped() {
        let items = to_items(
            &page(vec![
                issue("ABC-1", Some("2026-09-13T23:59:59.999+0200")),
                issue("ABC-2", Some("2026-09-14T00:00:00.000+0200")),
                issue("ABC-3", Some("2026-09-15T14:29:59.000+0200")),
                issue("ABC-4", Some("2026-09-15T14:30:00.000+0200")),
            ]),
            &site(),
            &range(),
            &tz(),
        );
        let keys: Vec<_> = items
            .iter()
            .filter_map(|i| i.reference.as_deref())
            .collect();
        assert_eq!(keys, vec!["ABC-2", "ABC-3"]);
    }

    #[test]
    fn unresolved_issues_are_dropped() {
        let items = to_items(&page(vec![issue("ABC-1", None)]), &site(), &range(), &tz());
        assert!(items.is_empty());
    }

    #[test]
    fn the_project_key_stands_in_for_a_missing_name() {
        let raw = serde_json::json!({ "issues": [{
            "key": "ABC-1",
            "fields": {
                "summary": "s",
                "resolutiondate": "2026-09-14T11:15:00.000+0200",
                "project": { "key": "ABC" }
            }
        }]});
        let items = to_items(&parse_page(&raw).unwrap(), &site(), &range(), &tz());
        assert_eq!(items[0].context.as_deref(), Some("ABC"));
    }

    #[test]
    fn a_malformed_timestamp_is_an_error_rather_than_a_silent_gap() {
        let raw = serde_json::json!({ "issues": [issue("ABC-1", Some("yesterday"))] });
        assert!(parse_page(&raw).is_err());
    }

    #[test]
    fn the_last_page_ends_pagination_even_with_a_token() {
        let last =
            parse_page(&serde_json::json!({ "issues": [], "nextPageToken": "t", "isLast": true }))
                .unwrap();
        assert_eq!(last.next(), None);
        let more = parse_page(&serde_json::json!({ "issues": [], "nextPageToken": "t" })).unwrap();
        assert_eq!(more.next(), Some("t"));
    }
}
