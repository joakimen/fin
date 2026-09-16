//! Output rendering.
//!
//! Rows are derived once, then handed to a renderer. Every renderer is a pure
//! function from rows to a `String`, so output is snapshot-testable without a
//! terminal.

pub mod json;
pub mod markdown;
pub mod terminal;

use jiff::Zoned;

use crate::config::Format;
use crate::item::Item;
use crate::period::TimeRange;

/// One rendered line of the report, with styling and layout still undecided.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub date: String,
    /// Human day heading, such as `Monday 14 September`.
    pub day: String,
    pub source: String,
    pub kind: String,
    pub reference: String,
    pub title: String,
    pub context: Option<String>,
    /// What the context is called in the item's source, such as `repository`.
    pub context_noun: &'static str,
    pub url: String,
}

/// Flattens items into rows, resolving every label a renderer might show.
pub fn rows(items: &[Item]) -> Vec<Row> {
    items
        .iter()
        .map(|item| Row {
            date: item.date(),
            day: day_label(&item.completed_at),
            source: source_label(item.source.as_str()),
            kind: kind_label(item.kind.as_str()),
            reference: item.reference.clone().unwrap_or_default(),
            title: item.title.clone(),
            context: item.context.clone(),
            context_noun: context_noun(item.source.as_str()),
            url: item.url.clone(),
        })
        .collect()
}

/// Renders a source identifier the way its own project spells it.
pub fn source_label(id: &str) -> String {
    match id {
        "github" => "GitHub".to_string(),
        "todoist" => "Todoist".to_string(),
        "jira" => "Jira".to_string(),
        other => capitalize(other),
    }
}

/// Names the place an item lives, in its source's own vocabulary.
pub fn context_noun(source: &str) -> &'static str {
    match source {
        "jira" => "project",
        _ => "repository",
    }
}

/// Renders an item kind as a noun, so a column of them reads as prose.
pub fn kind_label(kind: &str) -> String {
    match kind {
        "pr" => "PR".to_string(),
        other => capitalize(other),
    }
}

/// Renders the day heading a row is grouped under.
pub fn day_label(at: &Zoned) -> String {
    format!("{} {} {}", at.strftime("%A"), at.day(), at.strftime("%B"))
}

/// Renders a date the way a heading reads it, such as `Mon 14 Sep`.
pub fn short_date(at: &Zoned) -> String {
    format!("{} {} {}", at.strftime("%a"), at.day(), at.strftime("%b"))
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// Pluralizes a noun against a count.
pub fn plural(count: usize, singular: &str) -> String {
    if count == 1 {
        return format!("{count} {singular}");
    }
    let vowels = ['a', 'e', 'i', 'o', 'u'];
    let plural = match singular.chars().last() {
        Some('y')
            if !singular
                .chars()
                .nth(singular.len().saturating_sub(2))
                .is_some_and(|c| vowels.contains(&c.to_ascii_lowercase())) =>
        {
            format!("{}ies", &singular[..singular.len() - 1])
        }
        Some('s') | Some('x') => format!("{singular}es"),
        _ => format!("{singular}s"),
    };
    format!("{count} {plural}")
}

/// What the report should show, independent of format.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    pub show_source: bool,
    pub styled: bool,
    /// Terminal width, when stdout is a terminal whose size is known.
    pub width: Option<usize>,
}

/// Renders a complete report.
pub fn render(format: Format, items: &[Item], range: &TimeRange, layout: Layout) -> String {
    let rows = rows(items);
    match format {
        Format::Terminal => terminal::render(&rows, range, layout),
        Format::Markdown => markdown::render(&rows, layout.show_source),
        Format::Json => json::render(items),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::{Kind, SourceId};

    pub fn sample() -> Vec<Item> {
        vec![
            Item {
                completed_at: "2026-09-14T11:15:00+02:00[Europe/Oslo]".parse().unwrap(),
                source: SourceId::new("github"),
                kind: Kind::new("pr"),
                reference: Some("#12".into()),
                title: "Add retries to the upload path".into(),
                url: "https://github.com/o/r/pull/12".into(),
                context: Some("o/r".into()),
            },
            Item {
                completed_at: "2026-09-14T14:00:00+02:00[Europe/Oslo]".parse().unwrap(),
                source: SourceId::new("github"),
                kind: Kind::new("issue"),
                reference: Some("#7".into()),
                title: "Flaky integration test".into(),
                url: "https://github.com/o/r/issues/7".into(),
                context: Some("o/r".into()),
            },
            Item {
                completed_at: "2026-09-15T09:30:00+02:00[Europe/Oslo]".parse().unwrap(),
                source: SourceId::new("github"),
                kind: Kind::new("pr"),
                reference: Some("#13".into()),
                title: "Drop the | separator".into(),
                url: "https://github.com/other/repo/pull/13".into(),
                context: Some("other/repo".into()),
            },
        ]
    }

    #[test]
    fn rows_carry_resolved_labels() {
        let rows = rows(&sample());
        assert_eq!(rows[0].date, "2026-09-14");
        assert_eq!(rows[0].day, "Monday 14 September");
        assert_eq!(rows[0].kind, "PR");
        assert_eq!(rows[0].source, "GitHub");
        assert_eq!(rows[0].reference, "#12");
    }

    #[test]
    fn kinds_and_sources_render_as_their_projects_spell_them() {
        assert_eq!(kind_label("pr"), "PR");
        assert_eq!(kind_label("issue"), "Issue");
        assert_eq!(kind_label("task"), "Task");
        assert_eq!(source_label("github"), "GitHub");
        assert_eq!(source_label("linear"), "Linear");
    }

    #[test]
    fn plurals_read_correctly() {
        assert_eq!(plural(1, "PR"), "1 PR");
        assert_eq!(plural(2, "PR"), "2 PRs");
        assert_eq!(plural(1, "repository"), "1 repository");
        assert_eq!(plural(3, "repository"), "3 repositories");
        assert_eq!(plural(2, "issue"), "2 issues");
        assert_eq!(plural(0, "item"), "0 items");
        assert_eq!(plural(2, "day"), "2 days");
        assert_eq!(plural(2, "box"), "2 boxes");
        assert_eq!(plural(2, "status"), "2 statuses");
    }

    #[test]
    fn dates_render_for_headings() {
        let at: Zoned = "2026-09-14T11:15:00+02:00[Europe/Oslo]".parse().unwrap();
        assert_eq!(day_label(&at), "Monday 14 September");
        assert_eq!(short_date(&at), "Mon 14 Sep");
    }
}
