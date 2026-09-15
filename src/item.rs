//! The normalized unit of completed work.
//!
//! Every source converts its own representation into [`Item`]; sorting,
//! grouping and rendering operate only on this type. I/O-free.

use std::fmt;
use std::str::FromStr;

use jiff::Zoned;
use serde::{Serialize, Serializer};

/// Identifier of a data source, such as `github`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceId(String);

/// Source-defined item category, such as `pr`, `issue` or `task`.
///
/// Intentionally not a shared enum: adding a source must not require editing
/// a central type, and two sources may use the same word for different things.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Kind(String);

macro_rules! string_newtype {
    ($name:ident) => {
        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into().to_ascii_lowercase())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = std::convert::Infallible;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self::new(s))
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.0)
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                Ok(Self::new(String::deserialize(d)?))
            }
        }
    };
}

string_newtype!(SourceId);
string_newtype!(Kind);

/// A single piece of finished work, dated by whatever its source considers
/// completion (a merge, a close, a task tick).
#[derive(Debug, Clone, Serialize)]
pub struct Item {
    #[serde(serialize_with = "serialize_timestamp")]
    pub completed_at: Zoned,
    pub source: SourceId,
    pub kind: Kind,
    /// Source-local identifier shown beside the title, such as `#128`.
    pub reference: Option<String>,
    pub title: String,
    pub url: String,
    /// Where the item lives, such as a repository or project name.
    pub context: Option<String>,
}

fn serialize_timestamp<S: Serializer>(value: &Zoned, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&value.timestamp().to_string())
}

impl Item {
    /// Completion date in the item's own zone, as `YYYY-MM-DD`.
    pub fn date(&self) -> String {
        self.completed_at.strftime("%Y-%m-%d").to_string()
    }
}

/// Orders items oldest first, breaking ties on stable fields so that repeated
/// runs over the same data produce byte-identical output.
pub fn sort(items: &mut [Item]) {
    items.sort_by(|a, b| {
        a.completed_at
            .cmp(&b.completed_at)
            .then_with(|| a.source.cmp(&b.source))
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.title.cmp(&b.title))
            .then_with(|| a.url.cmp(&b.url))
    });
}

/// Removes items sharing a URL, keeping the first occurrence.
///
/// Sources may legitimately return the same item from more than one query;
/// GitHub issue lookups match on author and assignee separately.
pub fn dedupe(items: &mut Vec<Item>) {
    let mut seen = std::collections::HashSet::new();
    items.retain(|item| seen.insert(item.url.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(url: &str, ts: &str) -> Item {
        Item {
            completed_at: ts.parse().unwrap(),
            source: SourceId::new("github"),
            kind: Kind::new("pr"),
            reference: None,
            title: "t".into(),
            url: url.into(),
            context: None,
        }
    }

    #[test]
    fn kinds_and_sources_normalize_case() {
        assert_eq!(Kind::new("PR").as_str(), "pr");
        assert_eq!(SourceId::new("GitHub").as_str(), "github");
    }

    #[test]
    fn sort_orders_oldest_first() {
        let mut items = vec![
            item("b", "2026-09-10T12:00:00+02:00[Europe/Oslo]"),
            item("a", "2026-09-08T12:00:00+02:00[Europe/Oslo]"),
        ];
        sort(&mut items);
        assert_eq!(items[0].url, "a");
    }

    #[test]
    fn dedupe_keeps_first_of_each_url() {
        let mut items = vec![
            item("a", "2026-09-08T12:00:00+02:00[Europe/Oslo]"),
            item("a", "2026-09-09T12:00:00+02:00[Europe/Oslo]"),
            item("b", "2026-09-09T12:00:00+02:00[Europe/Oslo]"),
        ];
        dedupe(&mut items);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].date(), "2026-09-08");
    }
}
