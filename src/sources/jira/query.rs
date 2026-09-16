//! Construction of Jira search requests.
//!
//! Pure: a window plus a few settings in, JQL and request bodies out.
//!
//! JQL date literals have minute precision, carry no offset, and are read in
//! the time zone of the searching user's Jira profile, which is independent
//! of the machine running this program. The query window is therefore
//! widened by the largest UTC offset in use on either side, and results are
//! trimmed to the exact window once their timestamps are parsed.

use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

use crate::period::TimeRange;

/// Largest distance between any civil time zone and UTC.
const WIDEST_OFFSET: SignedDuration = SignedDuration::from_hours(14);

/// Largest page the search endpoint is asked for.
pub const PAGE_SIZE: u32 = 100;

pub const FIELDS: [&str; 3] = ["summary", "resolutiondate", "project"];

/// JQL for issues assigned to the searching user and resolved during a
/// window that contains `range` whatever the profile's time zone.
pub fn resolved_issues(range: &TimeRange, projects: &[String]) -> String {
    let start = literal(range.start.timestamp() - WIDEST_OFFSET);
    let end = literal(range.end.timestamp() + WIDEST_OFFSET + SignedDuration::from_mins(1));
    format!(
        "assignee = currentUser() AND resolutiondate >= \"{start}\" \
         AND resolutiondate < \"{end}\"{} ORDER BY resolutiondate ASC",
        project_filter(projects)
    )
}

/// The body of one `POST /rest/api/3/search/jql` request.
pub fn search_body(jql: &str, next_page_token: Option<&str>) -> Value {
    let mut body = json!({
        "jql": jql,
        "fields": FIELDS,
        "maxResults": PAGE_SIZE,
    });
    if let Some(token) = next_page_token {
        body["nextPageToken"] = json!(token);
    }
    body
}

/// Formats an instant as a JQL date literal, truncated to the minute.
fn literal(at: Timestamp) -> String {
    at.strftime("%Y/%m/%d %H:%M").to_string()
}

fn project_filter(projects: &[String]) -> String {
    let quoted: Vec<String> = projects
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(|p| format!("\"{}\"", p.replace('\\', r"\\").replace('"', "\\\"")))
        .collect();
    if quoted.is_empty() {
        return String::new();
    }
    format!(" AND project in ({})", quoted.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::period::{self as period, DayName, Period};
    use jiff::Zoned;

    fn range() -> TimeRange {
        let now: Zoned = "2026-09-15T14:30:20+02:00[Europe/Oslo]".parse().unwrap();
        period::resolve(&now, Period::Week, DayName::Mon, false).unwrap()
    }

    #[test]
    fn the_window_is_widened_by_the_widest_offset_on_both_sides() {
        // Start 2026-09-13T22:00Z less 14h; end 12:30:20Z plus 14h, rounded up.
        assert_eq!(
            resolved_issues(&range(), &[]),
            "assignee = currentUser() AND resolutiondate >= \"2026/09/13 08:00\" \
             AND resolutiondate < \"2026/09/16 02:31\" ORDER BY resolutiondate ASC"
        );
    }

    #[test]
    fn projects_are_quoted_and_escaped() {
        let projects = vec!["ABC".to_string(), " ".to_string(), "A \"B\"".to_string()];
        assert!(
            resolved_issues(&range(), &projects)
                .contains(r#" AND project in ("ABC", "A \"B\"") ORDER BY"#)
        );
    }

    #[test]
    fn the_first_page_request_carries_no_page_token() {
        let body = search_body("jql", None);
        assert_eq!(body["jql"], "jql");
        assert_eq!(body["maxResults"], PAGE_SIZE);
        assert_eq!(
            body["fields"],
            json!(["summary", "resolutiondate", "project"])
        );
        assert!(body.get("nextPageToken").is_none());
    }

    #[test]
    fn later_page_requests_carry_the_page_token() {
        assert_eq!(search_body("jql", Some("t1"))["nextPageToken"], "t1");
    }
}
