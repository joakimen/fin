//! Markdown table output, for pasting into issue trackers and wikis.
//!
//! Never styled: the output is meant to survive a copy-paste intact.

use super::Row;

/// Renders rows as a GitHub-flavoured markdown table with inline links.
pub fn render(rows: &[Row], show_source: bool) -> String {
    if rows.is_empty() {
        return String::new();
    }

    let mut headers = vec!["Date"];
    if show_source {
        headers.push("Source");
    }
    headers.extend(["Type", "Item"]);

    let mut out = String::new();
    out.push_str(&format!("| {} |\n", headers.join(" | ")));
    out.push_str(&format!("| {} |\n", vec!["---"; headers.len()].join(" | ")));

    for row in rows {
        let mut cells = vec![row.date.clone()];
        if show_source {
            cells.push(escape(&row.source));
        }
        cells.push(escape(&row.kind));
        cells.push(item_cell(row));
        out.push_str(&format!("| {} |\n", cells.join(" | ")));
    }
    out
}

fn item_cell(row: &Row) -> String {
    let label = if row.reference.is_empty() {
        row.title.clone()
    } else {
        format!("{} {}", row.reference, row.title)
    };
    let link = format!("[{}]({})", escape(&label), row.url);
    match &row.context {
        Some(context) => format!("{link} — {}", escape(context)),
        None => link,
    }
}

/// Escapes the characters that would otherwise break a table cell.
fn escape(text: &str) -> String {
    text.replace('\\', r"\\")
        .replace('|', r"\|")
        .replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::super::rows;
    use super::super::tests::sample;
    use super::*;

    #[test]
    fn renders_a_table_with_links_and_no_source_column() {
        let out = render(&rows(&sample()), false);
        insta::assert_snapshot!(out);
    }

    #[test]
    fn renders_a_source_column_when_asked() {
        let out = render(&rows(&sample()), true);
        assert!(out.starts_with("| Date | Source | Type | Item |"));
        assert!(out.contains("| GitHub |"));
    }

    #[test]
    fn pipes_in_titles_are_escaped_so_cells_survive() {
        let out = render(&rows(&sample()), false);
        assert!(out.contains(r"Drop the \| separator"));
        assert_eq!(out.lines().filter(|l| l.starts_with('|')).count(), 5);
    }

    #[test]
    fn no_rows_renders_nothing() {
        assert_eq!(render(&[], false), "");
    }
}
