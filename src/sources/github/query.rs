//! Construction of GitHub search queries.
//!
//! Pure: a window plus a few settings in, query strings out. GitHub's search
//! grammar has no disjunction across different qualifiers, so a request that
//! reads as one question — "issues I authored or was assigned" — becomes more
//! than one query, deduplicated by the caller.

use crate::config::IssueMatch;
use crate::period::TimeRange;

/// A GitHub search expression together with the item kind it retrieves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Search {
    pub kind: &'static str,
    pub query: String,
}

/// Queries returning pull requests the user merged during the window.
pub fn merged_prs(user: &str, range: &TimeRange, orgs: &[String]) -> Vec<Search> {
    let (start, end) = range.as_search_bounds();
    vec![Search {
        kind: "pr",
        query: format!(
            "is:pr author:{user} is:merged merged:{start}..{end}{}",
            org_filter(orgs)
        ),
    }]
}

/// Queries returning issues closed during the window that belong to the user.
///
/// GitHub has no "closed by" qualifier, so association is by authorship or
/// assignment; `IssueMatch::Either` needs one query for each.
pub fn closed_issues(
    user: &str,
    range: &TimeRange,
    orgs: &[String],
    matching: IssueMatch,
) -> Vec<Search> {
    let (start, end) = range.as_search_bounds();
    let orgs = org_filter(orgs);

    let qualifiers: &[&str] = match matching {
        IssueMatch::Author => &["author"],
        IssueMatch::Assignee => &["assignee"],
        IssueMatch::Either => &["author", "assignee"],
    };

    qualifiers
        .iter()
        .map(|qualifier| Search {
            kind: "issue",
            query: format!("is:issue {qualifier}:{user} is:closed closed:{start}..{end}{orgs}"),
        })
        .collect()
}

/// Renders an organization restriction.
///
/// Repeating a single qualifier is how GitHub search expresses alternatives,
/// so several organizations widen the result rather than narrowing it to none.
fn org_filter(orgs: &[String]) -> String {
    orgs.iter()
        .filter(|org| !org.trim().is_empty())
        .map(|org| format!(" org:{org}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::period::{self as period, DayName, Period};
    use jiff::Zoned;

    fn range() -> TimeRange {
        let now: Zoned = "2026-09-15T14:30:00+02:00[Europe/Oslo]".parse().unwrap();
        period::resolve(&now, Period::Week, DayName::Mon, false).unwrap()
    }

    #[test]
    fn merged_pr_query_bounds_on_merge_time() {
        let q = merged_prs("joakimen", &range(), &[]);
        assert_eq!(q.len(), 1);
        assert_eq!(
            q[0].query,
            "is:pr author:joakimen is:merged \
             merged:2026-09-14T00:00:00+02:00..2026-09-15T14:30:00+02:00"
        );
    }

    #[test]
    fn either_match_produces_one_query_per_qualifier() {
        let q = closed_issues("joakimen", &range(), &[], IssueMatch::Either);
        assert_eq!(q.len(), 2);
        assert!(q[0].query.contains("author:joakimen"));
        assert!(q[1].query.contains("assignee:joakimen"));
        assert!(q.iter().all(|s| s.kind == "issue"));
    }

    #[test]
    fn narrower_match_produces_a_single_query() {
        assert_eq!(
            closed_issues("j", &range(), &[], IssueMatch::Author).len(),
            1
        );
        assert_eq!(
            closed_issues("j", &range(), &[], IssueMatch::Assignee).len(),
            1
        );
    }

    #[test]
    fn organizations_are_appended_as_repeated_qualifiers() {
        let orgs = vec!["one".to_string(), "two".to_string()];
        let q = merged_prs("j", &range(), &orgs);
        assert!(q[0].query.ends_with(" org:one org:two"));
    }

    #[test]
    fn blank_organization_entries_are_skipped() {
        let orgs = vec!["  ".to_string(), "real".to_string()];
        assert_eq!(org_filter(&orgs), " org:real");
        assert_eq!(org_filter(&[]), "");
    }
}
