//! On-disk response cache with a time-to-live.
//!
//! Repeating a report within the TTL should not re-query anything: the same
//! window is asked for repeatedly while a report is being written, and every
//! query costs rate limit and a visible pause.
//!
//! Caching is applied by wrapping a source in [`Caching`], so sources stay
//! unaware of it. The key derivation and freshness test are pure; only
//! [`Cache`] itself touches the filesystem.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use jiff::{Timestamp, Zoned};
use serde::{Deserialize, Serialize};

use crate::diag::Diag;
use crate::item::{Item, Kind, SourceId};
use crate::period::TimeRange;
use crate::source::{Fetch, Source};

/// Bumped when the on-disk entry shape changes, so old entries are ignored
/// rather than misread.
const FORMAT_VERSION: &str = "v1";

/// Resolves the cache directory.
///
/// Precedence: `XDG_CACHE_HOME`, then `~/.cache`.
pub fn cache_dir(xdg: Option<&str>, home: Option<&str>) -> Result<PathBuf> {
    let base = match xdg.filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => {
            let home = home.filter(|v| !v.is_empty()).context("HOME is not set")?;
            PathBuf::from(home).join(".cache")
        }
    };
    Ok(base.join("fin").join(FORMAT_VERSION))
}

/// Derives a stable cache key from the inputs that change a result.
///
/// An open-ended window contributes only its start: its end is the current
/// moment and would make every key unique, defeating the cache entirely. The
/// TTL is what bounds staleness in that case.
pub fn key(source: &SourceId, fingerprint: &str, range: &TimeRange, kinds: &[Kind]) -> String {
    let mut sorted: Vec<&str> = kinds.iter().map(Kind::as_str).collect();
    sorted.sort_unstable();

    let end = if range.open_ended {
        "open".to_string()
    } else {
        range.end.timestamp().to_string()
    };

    let material = format!(
        "{source}\u{1f}{fingerprint}\u{1f}{}\u{1f}{end}\u{1f}{}",
        range.start.timestamp(),
        sorted.join(",")
    );
    format!("{source}-{}", fnv1a_hex(&material))
}

/// FNV-1a, 64-bit. Chosen over the standard hasher because the digest must
/// stay identical across builds for entries to survive an upgrade.
fn fnv1a_hex(input: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Whether an entry written at `fetched_at` may still be served.
pub fn is_fresh(fetched_at: Timestamp, now: Timestamp, ttl: Duration) -> bool {
    if ttl.is_zero() {
        return false;
    }
    let age = now
        .as_millisecond()
        .saturating_sub(fetched_at.as_millisecond());
    if age < 0 {
        return false;
    }
    u128::try_from(age).unwrap_or(u128::MAX) < ttl.as_millis()
}

/// Cache-owned item shape.
///
/// Distinct from the JSON the tool prints: an entry stores the full zoned
/// timestamp so a restored item is byte-identical to a fetched one, whereas
/// the printed form is a plain UTC instant.
#[derive(Serialize, Deserialize)]
struct CachedItem {
    completed_at: Zoned,
    source: SourceId,
    kind: Kind,
    reference: Option<String>,
    title: String,
    url: String,
    context: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    fetched_at: Timestamp,
    items: Vec<CachedItem>,
    #[serde(default)]
    warnings: Vec<String>,
}

impl From<&Item> for CachedItem {
    fn from(item: &Item) -> Self {
        Self {
            completed_at: item.completed_at.clone(),
            source: item.source.clone(),
            kind: item.kind.clone(),
            reference: item.reference.clone(),
            title: item.title.clone(),
            url: item.url.clone(),
            context: item.context.clone(),
        }
    }
}

impl From<CachedItem> for Item {
    fn from(cached: CachedItem) -> Self {
        Self {
            completed_at: cached.completed_at,
            source: cached.source,
            kind: cached.kind,
            reference: cached.reference,
            title: cached.title,
            url: cached.url,
            context: cached.context,
        }
    }
}

pub struct Cache {
    dir: PathBuf,
    ttl: Duration,
}

impl Cache {
    pub fn new(dir: PathBuf, ttl: Duration) -> Self {
        Self { dir, ttl }
    }

    pub fn enabled(&self) -> bool {
        !self.ttl.is_zero()
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }

    /// Returns a cached result when an entry exists and is still fresh.
    ///
    /// A missing, unreadable or stale entry is a miss rather than an error: a
    /// broken cache must never be able to fail a report.
    pub fn read(&self, key: &str, now: Timestamp) -> Option<Fetch> {
        if !self.enabled() {
            return None;
        }
        let text = std::fs::read_to_string(self.path(key)).ok()?;
        let entry: Entry = serde_json::from_str(&text).ok()?;
        if !is_fresh(entry.fetched_at, now, self.ttl) {
            return None;
        }
        Some(Fetch {
            items: entry.items.into_iter().map(Item::from).collect(),
            warnings: entry.warnings,
        })
    }

    /// Stores a result against a key.
    pub fn write(&self, key: &str, fetch: &Fetch, now: Timestamp) -> Result<()> {
        if !self.enabled() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("cannot create {}", self.dir.display()))?;

        let entry = Entry {
            fetched_at: now,
            items: fetch.items.iter().map(CachedItem::from).collect(),
            warnings: fetch.warnings.clone(),
        };
        let text = serde_json::to_string(&entry).context("cannot encode a cache entry")?;

        // Written beside the target and renamed so a concurrent reader sees
        // either the old entry or the new one, never a partial file.
        let temporary = self.path(&format!("{key}.tmp{}", std::process::id()));
        std::fs::write(&temporary, text)
            .with_context(|| format!("cannot write {}", temporary.display()))?;
        std::fs::rename(&temporary, self.path(key))
            .with_context(|| format!("cannot replace {}", self.path(key).display()))?;
        Ok(())
    }

    /// Removes every entry, returning how many were deleted.
    pub fn clear(&self) -> Result<usize> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", self.dir.display())),
        };

        let mut removed = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") && std::fs::remove_file(&path).is_ok()
            {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

/// A source that serves recent results from disk before asking upstream.
pub struct Caching {
    inner: Box<dyn Source>,
    cache: Cache,
    diag: Diag,
}

impl Caching {
    pub fn new(inner: Box<dyn Source>, cache: Cache, diag: Diag) -> Self {
        Self { inner, cache, diag }
    }
}

#[async_trait]
impl Source for Caching {
    fn id(&self) -> SourceId {
        self.inner.id()
    }

    fn supported_kinds(&self) -> Vec<Kind> {
        self.inner.supported_kinds()
    }

    fn cache_fingerprint(&self) -> String {
        self.inner.cache_fingerprint()
    }

    async fn fetch(&self, range: &TimeRange, kinds: &[Kind]) -> Result<Fetch> {
        let key = key(&self.id(), &self.cache_fingerprint(), range, kinds);
        let now = Timestamp::now();

        if let Some(fetch) = self.cache.read(&key, now) {
            self.diag.log(format_args!(
                "{} cache hit ({key}): {} items",
                self.id(),
                fetch.items.len()
            ));
            return Ok(fetch);
        }
        self.diag
            .log(format_args!("{} cache miss ({key})", self.id()));

        let fetch = self.inner.fetch(range, kinds).await?;

        if let Err(e) = self.cache.write(&key, &fetch, now) {
            self.diag
                .log(format_args!("{} cache write failed: {e}", self.id()));
        }
        Ok(fetch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::period::{self as period, DayName, Period};

    fn range(previous: bool) -> TimeRange {
        let now: Zoned = "2026-09-15T14:30:00+02:00[Europe/Oslo]".parse().unwrap();
        period::resolve(&now, Period::Week, DayName::Mon, previous).unwrap()
    }

    fn kinds() -> Vec<Kind> {
        vec![Kind::new("pr"), Kind::new("issue")]
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fin-cache-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn fetch(urls: &[&str]) -> Fetch {
        Fetch {
            items: urls.iter().map(|url| item(url)).collect(),
            warnings: Vec::new(),
        }
    }

    fn item(url: &str) -> Item {
        Item {
            completed_at: "2026-09-14T11:15:00+02:00[Europe/Oslo]".parse().unwrap(),
            source: SourceId::new("github"),
            kind: Kind::new("pr"),
            reference: Some("#1".into()),
            title: "Work".into(),
            url: url.into(),
            context: Some("o/r".into()),
        }
    }

    #[test]
    fn the_same_inputs_produce_the_same_key() {
        let id = SourceId::new("github");
        let a = key(&id, "fp", &range(false), &kinds());
        let b = key(&id, "fp", &range(false), &kinds());
        assert_eq!(a, b);
        assert!(a.starts_with("github-"));
    }

    #[test]
    fn kind_order_does_not_change_the_key() {
        let id = SourceId::new("github");
        let forward = key(
            &id,
            "fp",
            &range(false),
            &[Kind::new("pr"), Kind::new("issue")],
        );
        let reverse = key(
            &id,
            "fp",
            &range(false),
            &[Kind::new("issue"), Kind::new("pr")],
        );
        assert_eq!(forward, reverse);
    }

    #[test]
    fn a_changed_fingerprint_changes_the_key() {
        let id = SourceId::new("github");
        assert_ne!(
            key(&id, "orgs=", &range(false), &kinds()),
            key(&id, "orgs=acme", &range(false), &kinds())
        );
    }

    #[test]
    fn differing_kinds_change_the_key() {
        let id = SourceId::new("github");
        assert_ne!(
            key(&id, "fp", &range(false), &[Kind::new("pr")]),
            key(&id, "fp", &range(false), &kinds())
        );
    }

    #[test]
    fn an_open_window_ignores_its_moving_end() {
        let id = SourceId::new("github");
        let early: Zoned = "2026-09-15T09:00:00+02:00[Europe/Oslo]".parse().unwrap();
        let late: Zoned = "2026-09-15T17:00:00+02:00[Europe/Oslo]".parse().unwrap();
        let a = period::resolve(&early, Period::Week, DayName::Mon, false).unwrap();
        let b = period::resolve(&late, Period::Week, DayName::Mon, false).unwrap();
        assert_ne!(a.end, b.end);
        assert_eq!(key(&id, "fp", &a, &kinds()), key(&id, "fp", &b, &kinds()));
    }

    #[test]
    fn a_closed_window_keys_on_its_end() {
        let id = SourceId::new("github");
        assert_ne!(
            key(&id, "fp", &range(false), &kinds()),
            key(&id, "fp", &range(true), &kinds())
        );
    }

    #[test]
    fn the_digest_is_stable_across_builds() {
        assert_eq!(fnv1a_hex(""), "cbf29ce484222325");
        assert_eq!(fnv1a_hex("a"), "af63dc4c8601ec8c");
    }

    #[test]
    fn freshness_is_bounded_by_the_ttl() {
        let base: Timestamp = "2026-09-15T12:00:00Z".parse().unwrap();
        let ttl = Duration::from_secs(900);
        let later: Timestamp = "2026-09-15T12:14:59Z".parse().unwrap();
        let expired: Timestamp = "2026-09-15T12:15:01Z".parse().unwrap();
        assert!(is_fresh(base, later, ttl));
        assert!(!is_fresh(base, expired, ttl));
    }

    #[test]
    fn a_zero_ttl_never_serves_an_entry() {
        let base: Timestamp = "2026-09-15T12:00:00Z".parse().unwrap();
        assert!(!is_fresh(base, base, Duration::ZERO));
    }

    #[test]
    fn an_entry_from_the_future_is_not_served() {
        let base: Timestamp = "2026-09-15T12:00:00Z".parse().unwrap();
        let earlier: Timestamp = "2026-09-15T11:00:00Z".parse().unwrap();
        assert!(!is_fresh(base, earlier, Duration::from_secs(900)));
    }

    #[test]
    fn a_written_entry_reads_back_unchanged() {
        let cache = Cache::new(temp_dir("roundtrip"), Duration::from_secs(900));
        let now: Timestamp = "2026-09-15T12:00:00Z".parse().unwrap();
        let mut written = fetch(&["https://example.test/1"]);
        written.warnings.push("capped".into());

        cache.write("k", &written, now).unwrap();
        let restored = cache.read("k", now).unwrap();

        assert_eq!(restored.items.len(), 1);
        assert_eq!(restored.items[0].url, written.items[0].url);
        assert_eq!(
            restored.items[0].completed_at,
            written.items[0].completed_at
        );
        assert_eq!(restored.items[0].reference, written.items[0].reference);
        assert_eq!(restored.warnings, written.warnings);
    }

    #[test]
    fn an_entry_written_without_warnings_still_reads() {
        let dir = temp_dir("no-warnings");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("k.json"),
            r#"{"fetched_at":"2026-09-15T12:00:00Z","items":[]}"#,
        )
        .unwrap();
        let cache = Cache::new(dir, Duration::from_secs(900));
        let now: Timestamp = "2026-09-15T12:00:00Z".parse().unwrap();
        let restored = cache.read("k", now).unwrap();
        assert!(restored.items.is_empty());
        assert!(restored.warnings.is_empty());
    }

    #[test]
    fn a_stale_entry_is_a_miss() {
        let cache = Cache::new(temp_dir("stale"), Duration::from_secs(60));
        let written: Timestamp = "2026-09-15T12:00:00Z".parse().unwrap();
        let much_later: Timestamp = "2026-09-15T13:00:00Z".parse().unwrap();
        cache
            .write("k", &fetch(&["https://example.test/1"]), written)
            .unwrap();
        assert!(cache.read("k", much_later).is_none());
    }

    #[test]
    fn a_missing_entry_is_a_miss_rather_than_an_error() {
        let cache = Cache::new(temp_dir("absent"), Duration::from_secs(900));
        let now: Timestamp = "2026-09-15T12:00:00Z".parse().unwrap();
        assert!(cache.read("nothing-here", now).is_none());
    }

    #[test]
    fn a_corrupt_entry_is_a_miss_rather_than_an_error() {
        let dir = temp_dir("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("k.json"), "{ not json").unwrap();
        let cache = Cache::new(dir, Duration::from_secs(900));
        let now: Timestamp = "2026-09-15T12:00:00Z".parse().unwrap();
        assert!(cache.read("k", now).is_none());
    }

    #[test]
    fn a_disabled_cache_neither_reads_nor_writes() {
        let dir = temp_dir("disabled");
        let cache = Cache::new(dir.clone(), Duration::ZERO);
        let now: Timestamp = "2026-09-15T12:00:00Z".parse().unwrap();
        cache
            .write("k", &fetch(&["https://example.test/1"]), now)
            .unwrap();
        assert!(
            !dir.exists(),
            "a disabled cache must not create its directory"
        );
        assert!(cache.read("k", now).is_none());
    }

    #[test]
    fn clearing_removes_entries_and_counts_them() {
        let cache = Cache::new(temp_dir("clear"), Duration::from_secs(900));
        let now: Timestamp = "2026-09-15T12:00:00Z".parse().unwrap();
        cache
            .write("a", &fetch(&["https://example.test/1"]), now)
            .unwrap();
        cache
            .write("b", &fetch(&["https://example.test/2"]), now)
            .unwrap();

        assert_eq!(cache.clear().unwrap(), 2);
        assert!(cache.read("a", now).is_none());
    }

    #[test]
    fn clearing_an_absent_directory_is_not_an_error() {
        let cache = Cache::new(temp_dir("never-created"), Duration::from_secs(900));
        assert_eq!(cache.clear().unwrap(), 0);
    }

    #[test]
    fn writing_leaves_no_temporary_files_behind() {
        let dir = temp_dir("tidy");
        let cache = Cache::new(dir.clone(), Duration::from_secs(900));
        let now: Timestamp = "2026-09-15T12:00:00Z".parse().unwrap();
        cache
            .write("k", &fetch(&["https://example.test/1"]), now)
            .unwrap();

        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temporary files remained: {leftovers:?}"
        );
    }

    #[test]
    fn cache_dir_follows_xdg_then_home() {
        assert_eq!(
            cache_dir(Some("/xdg"), Some("/home")).unwrap(),
            PathBuf::from("/xdg/fin/v1")
        );
        assert_eq!(
            cache_dir(None, Some("/home")).unwrap(),
            PathBuf::from("/home/.cache/fin/v1")
        );
        assert!(cache_dir(None, None).is_err());
    }
}
