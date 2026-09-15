//! JSON output, for piping into other tools.

use crate::item::Item;

/// Renders items as a JSON array, one object per item.
pub fn render(items: &[Item]) -> String {
    serde_json::to_string_pretty(items).unwrap_or_else(|_| "[]".to_string())
}

#[cfg(test)]
mod tests {
    use super::super::tests::sample;
    use super::*;

    #[test]
    fn renders_an_array_of_items_with_utc_timestamps() {
        let out = render(&sample());
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed.as_array().unwrap().len(), 3);
        assert_eq!(parsed[0]["kind"], "pr");
        assert_eq!(parsed[0]["source"], "github");
        assert_eq!(parsed[0]["completed_at"], "2026-09-14T09:15:00Z");
    }

    #[test]
    fn no_items_renders_an_empty_array() {
        assert_eq!(render(&[]), "[]");
    }
}
