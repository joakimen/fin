//! Styled terminal output.
//!
//! Rows are grouped under a heading per day, because a work report is read a
//! day at a time. Columns are laid out on the plain text and styled
//! afterwards: colour and hyperlink escapes occupy no display width and would
//! otherwise corrupt every alignment they touch.
//!
//! Only the sixteen themeable ANSI colours are used, so the terminal's own
//! theme resolves them and the output stays legible on light and dark
//! backgrounds alike.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use anstyle::{AnsiColor, Style};
use unicode_width::UnicodeWidthStr;

use super::{Layout, Row, plural, short_date};
use crate::period::TimeRange;

const INDENT: &str = "  ";
const GAP: usize = 2;
const MIN_TITLE: usize = 24;
const MAX_RULE: usize = 96;

fn title_style() -> Style {
    Style::new().bold()
}

/// Structure is carried by weight and space, not colour, so headings stay
/// legible under any theme and the body keeps only two hues.
fn subtitle_style() -> Style {
    Style::new()
}

fn day_style() -> Style {
    Style::new().bold()
}

/// Identifiers and locations are metadata around the title, and recede.
fn reference_style() -> Style {
    Style::new().fg_color(Some(AnsiColor::Magenta.into()))
}

fn context_style() -> Style {
    Style::new().fg_color(Some(AnsiColor::Magenta.into()))
}

fn rule_style() -> Style {
    Style::new()
}

/// Colour for an item kind.
///
/// The kind is always spelled out beside it, so the colour is redundant
/// decoration rather than the only carrier of meaning.
fn kind_style(kind: &str) -> Style {
    let colour = match kind {
        "PR" => AnsiColor::Cyan,
        "Issue" => AnsiColor::Green,
        _ => AnsiColor::Yellow,
    };
    Style::new().fg_color(Some(colour.into()))
}

/// Wraps text in an OSC 8 hyperlink.
///
/// Adds no styling of its own: terminals mark hyperlinks themselves, often
/// according to a user preference that an underline here would override.
fn hyperlink(url: &str, text: &str) -> String {
    format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
}

fn paint(style: Style, text: &str, styled: bool) -> String {
    if styled {
        format!("{style}{text}{style:#}")
    } else {
        text.to_owned()
    }
}

/// Renders the report: a heading, a block per day, and a summary.
pub fn render(rows: &[Row], range: &TimeRange, layout: Layout) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{}",
        paint(title_style(), "Completed work", layout.styled)
    );

    let subtitle = match rows.len() {
        0 => window_label(range),
        n => format!("{} · {}", window_label(range), plural(n, "item")),
    };
    let _ = writeln!(out, "{}", paint(subtitle_style(), &subtitle, layout.styled));
    out.push('\n');

    if rows.is_empty() {
        let _ = writeln!(out, "Nothing was completed in this period.");
        return out;
    }

    let widths = Widths::measure(rows, &layout);

    let mut current_day = String::new();
    for row in rows {
        if row.day != current_day {
            if !current_day.is_empty() {
                out.push('\n');
            }
            let _ = writeln!(out, "{}", paint(day_style(), &row.day, layout.styled));
            current_day = row.day.clone();
        }
        let _ = writeln!(out, "{}", render_row(row, &widths, &layout));
    }

    out.push('\n');
    let _ = writeln!(
        out,
        "{}",
        paint(rule_style(), &"─".repeat(widths.rule), layout.styled)
    );
    let _ = writeln!(out, "{}", summary(rows, layout.styled));
    out
}

/// Renders the window as a heading reads it, collapsing a single day.
fn window_label(range: &TimeRange) -> String {
    let start = short_date(&range.start);
    let last = range.end.clone() - jiff::Span::new().seconds(1);
    let end = short_date(&last);
    if start == end {
        return format!("{start} {}", last.year());
    }
    format!("{start} – {end} {}", last.year())
}

struct Widths {
    kind: usize,
    source: usize,
    reference: usize,
    title: Option<usize>,
    rule: usize,
}

impl Widths {
    fn measure(rows: &[Row], layout: &Layout) -> Self {
        let kind = max_width(rows.iter().map(|r| r.kind.as_str()));
        let source = if layout.show_source {
            max_width(rows.iter().map(|r| r.source.as_str())) + GAP
        } else {
            0
        };
        let reference = max_width(rows.iter().map(|r| r.reference.as_str()));
        let context = max_width(rows.iter().filter_map(|r| r.context.as_deref()));
        let natural_title = max_width(rows.iter().map(|r| r.title.as_str()));

        let fixed = INDENT.width() + kind + GAP + source + reference + GAP;

        // Without a known width nothing is truncated; a wrapped line is a
        // better failure than a silently shortened title.
        let (title, rule) = match layout.width {
            None => (None, (fixed + natural_title + GAP + context).min(MAX_RULE)),
            Some(available) => {
                let room = available.saturating_sub(fixed + GAP + context);
                let title = natural_title.min(room.max(MIN_TITLE));
                (Some(title), (fixed + title + GAP + context).min(available))
            }
        };

        Self {
            kind,
            source,
            reference,
            title,
            rule,
        }
    }
}

fn render_row(row: &Row, widths: &Widths, layout: &Layout) -> String {
    let mut line = String::from(INDENT);

    push_cell(
        &mut line,
        &paint(kind_style(&row.kind), &row.kind, layout.styled),
        row.kind.width(),
        widths.kind + GAP,
    );

    if layout.show_source {
        push_cell(&mut line, &row.source, row.source.width(), widths.source);
    }

    push_cell(
        &mut line,
        &paint(reference_style(), &row.reference, layout.styled),
        row.reference.width(),
        widths.reference + GAP,
    );

    let title = match widths.title {
        Some(limit) => truncate(&row.title, limit),
        None => row.title.clone(),
    };
    let title_width = title.width();
    let rendered = if layout.styled {
        hyperlink(&row.url, &title)
    } else {
        title
    };

    match (&row.context, widths.title) {
        (None, _) => line.push_str(&rendered),
        (Some(context), Some(limit)) => {
            push_cell(&mut line, &rendered, title_width, limit + GAP);
            line.push_str(&paint(context_style(), context, layout.styled));
        }
        (Some(context), None) => {
            line.push_str(&rendered);
            line.push_str("  ");
            line.push_str(&paint(context_style(), context, layout.styled));
        }
    }

    line.trim_end().to_string()
}

/// Renders totals: how much, of what, across how many places.
fn summary(rows: &[Row], styled: bool) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for row in rows {
        *counts.entry(row.kind.as_str()).or_default() += 1;
    }
    let mut places: BTreeMap<&str, std::collections::BTreeSet<&str>> = BTreeMap::new();
    for row in rows {
        if let Some(context) = row.context.as_deref() {
            places.entry(row.context_noun).or_default().insert(context);
        }
    }

    let mut parts = vec![paint(
        Style::new().bold(),
        &plural(rows.len(), "item"),
        styled,
    )];
    parts.extend(
        counts
            .iter()
            .map(|(kind, count)| paint(kind_style(kind), &plural(*count, kind), styled)),
    );
    parts.extend(
        places
            .iter()
            .map(|(noun, names)| paint(context_style(), &plural(names.len(), noun), styled)),
    );
    parts.join(" · ")
}

fn max_width<'a>(values: impl Iterator<Item = &'a str>) -> usize {
    values.map(UnicodeWidthStr::width).max().unwrap_or(0)
}

/// Shortens text to `limit` display columns, marking the cut.
fn truncate(text: &str, limit: usize) -> String {
    if text.width() <= limit {
        return text.to_owned();
    }
    if limit <= 1 {
        return "…".to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.to_string().width();
        if used + w > limit - 1 {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

/// Appends `text`, padding to `width` using `display_width` rather than the
/// string's length, so styled text still aligns.
fn push_cell(line: &mut String, text: &str, display_width: usize, width: usize) {
    line.push_str(text);
    for _ in 0..width.saturating_sub(display_width) {
        line.push(' ');
    }
}

#[cfg(test)]
mod tests {
    use super::super::rows;
    use super::super::tests::sample;
    use super::*;
    use crate::period::{self as period, DayName, Period};
    use jiff::Zoned;

    fn range() -> TimeRange {
        let now: Zoned = "2026-09-15T14:30:00+02:00[Europe/Oslo]".parse().unwrap();
        period::resolve(&now, Period::Week, DayName::Mon, false).unwrap()
    }

    fn plain() -> Layout {
        Layout {
            show_source: false,
            styled: false,
            width: Some(80),
        }
    }

    /// Removes CSI colour sequences and OSC 8 hyperlink wrappers, leaving the
    /// text a terminal would actually display.
    fn strip_escapes(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '\x1b' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('[') => {
                    for t in chars.by_ref() {
                        if ('@'..='~').contains(&t) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(t) = chars.next() {
                        if t == '\x07' {
                            break;
                        }
                        if t == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    #[test]
    fn renders_a_grouped_plain_report() {
        insta::assert_snapshot!(render(&rows(&sample()), &range(), plain()));
    }

    #[test]
    fn renders_a_styled_report() {
        let layout = Layout {
            show_source: false,
            styled: true,
            width: Some(80),
        };
        insta::assert_snapshot!(render(&rows(&sample()), &range(), layout));
    }

    #[test]
    fn rows_are_grouped_under_one_heading_per_day() {
        let out = render(&rows(&sample()), &range(), plain());
        assert_eq!(out.matches("Monday 14 September").count(), 1);
        assert_eq!(out.matches("Tuesday 15 September").count(), 1);
    }

    #[test]
    fn styling_never_changes_the_visible_layout() {
        let styled = render(
            &rows(&sample()),
            &range(),
            Layout {
                styled: true,
                ..plain()
            },
        );
        assert_eq!(
            strip_escapes(&styled),
            render(&rows(&sample()), &range(), plain())
        );
    }

    #[test]
    fn styled_output_hyperlinks_each_title() {
        let out = render(
            &rows(&sample()),
            &range(),
            Layout {
                styled: true,
                ..plain()
            },
        );
        assert!(out.contains("\x1b]8;;https://github.com/o/r/pull/12\x1b\\"));
    }

    #[test]
    fn titles_carry_no_styling_of_their_own() {
        let out = render(
            &rows(&sample()),
            &range(),
            Layout {
                styled: true,
                ..plain()
            },
        );
        assert!(
            !out.contains("\x1b[4m"),
            "a hyperlink must not hardcode an underline: the terminal marks it"
        );

        let title = "Add retries to the upload path";
        let at = out.find(title).expect("title missing from output");
        assert!(
            out[..at].ends_with("\x1b\\"),
            "title is preceded by styling rather than by the hyperlink opener"
        );
    }

    #[test]
    fn unstyled_output_has_no_escape_sequences() {
        let out = render(&rows(&sample()), &range(), plain());
        assert!(!out.contains('\x1b'), "escape leaked into unstyled output");
    }

    #[test]
    fn the_summary_counts_items_kinds_and_repositories() {
        let out = render(&rows(&sample()), &range(), plain());
        assert!(
            out.contains("3 items · 1 Issue · 2 PRs · 2 repositories"),
            "got:\n{out}"
        );
    }

    #[test]
    fn the_summary_counts_places_in_each_sources_own_vocabulary() {
        let mut items = sample();
        let mut ticket = items[0].clone();
        ticket.source = crate::item::SourceId::new("jira");
        ticket.kind = crate::item::Kind::new("issue");
        ticket.url = "https://example.atlassian.net/browse/ABC-1".into();
        items.push(ticket);

        let out = render(&rows(&items), &range(), plain());
        assert!(
            out.contains("4 items · 2 Issues · 2 PRs · 1 project · 2 repositories"),
            "got:\n{out}"
        );
    }

    #[test]
    fn a_source_column_appears_when_more_than_one_source_is_in_play() {
        let out = render(
            &rows(&sample()),
            &range(),
            Layout {
                show_source: true,
                ..plain()
            },
        );
        assert!(out.contains("GitHub"));
    }

    #[test]
    fn an_empty_report_says_so_without_a_summary() {
        let out = render(&[], &range(), plain());
        assert!(out.contains("Nothing was completed in this period."));
        assert!(!out.contains("items"));
    }

    #[test]
    fn titles_are_truncated_to_the_terminal_width() {
        let mut items = sample();
        items[0].title = "A".repeat(200);
        let narrow = Layout {
            show_source: false,
            styled: false,
            width: Some(60),
        };
        let out = render(&rows(&items), &range(), narrow);
        assert!(out.contains('…'), "long title was not truncated");
        assert!(
            out.lines().all(|l| l.width() <= 60),
            "a line overflowed the terminal"
        );
    }

    #[test]
    fn an_unknown_width_leaves_titles_intact() {
        let mut items = sample();
        items[0].title = "A".repeat(200);
        let unbounded = Layout {
            show_source: false,
            styled: false,
            width: None,
        };
        let out = render(&rows(&items), &range(), unbounded);
        assert!(out.contains(&"A".repeat(200)));
        assert!(!out.contains('…'));
    }

    #[test]
    fn a_single_day_window_reads_as_one_date() {
        let now: Zoned = "2026-09-14T14:30:00+02:00[Europe/Oslo]".parse().unwrap();
        let single = period::resolve(&now, Period::Week, DayName::Mon, false).unwrap();
        assert_eq!(window_label(&single), "Mon 14 Sep 2026");
    }

    #[test]
    fn a_multi_day_window_names_both_ends() {
        assert_eq!(window_label(&range()), "Mon 14 Sep – Tue 15 Sep 2026");
    }

    #[test]
    fn truncation_marks_the_cut_and_respects_wide_characters() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 8), "hello w…");
        assert_eq!(truncate("日本語のタイトル", 7), "日本語…");
        assert_eq!(truncate("anything", 1), "…");
    }

    #[test]
    fn wide_characters_keep_columns_aligned() {
        let mut items = sample();
        items[0].title = "日本語のタイトル".into();
        let out = render(&rows(&items), &range(), plain());
        let body: Vec<&str> = out.lines().filter(|l| l.starts_with("  ")).collect();
        // Measured in display columns, not bytes: a wide character occupies
        // two columns but three bytes, so a byte offset would differ between
        // rows that are in fact aligned.
        let column_of =
            |line: &str, needle: &str| line.find(needle).map(|byte| line[..byte].width());
        let context_columns: Vec<Option<usize>> = body
            .iter()
            .map(|l| column_of(l, "o/r").or_else(|| column_of(l, "other/repo")))
            .collect();
        assert!(
            context_columns.windows(2).all(|w| w[0] == w[1]),
            "context column drifted: {context_columns:?}"
        );
    }
}
