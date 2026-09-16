//! The data-source abstraction.
//!
//! A source knows how to ask one upstream system for work completed in a
//! window and to normalize the answer into [`Item`]s. Sources perform I/O;
//! the conversion of a response into items belongs in a pure function that
//! the source calls.

use anyhow::Result;
use async_trait::async_trait;
use futures::future;

use crate::item::{Item, Kind, SourceId};
use crate::period::TimeRange;

#[async_trait]
pub trait Source: Send + Sync {
    fn id(&self) -> SourceId;

    /// Item kinds this source can return, used to validate configuration and
    /// to render help.
    fn supported_kinds(&self) -> Vec<Kind>;

    /// Identity of the settings that change this source's results, so a
    /// configuration change invalidates cached entries.
    fn cache_fingerprint(&self) -> String;

    async fn fetch(&self, range: &TimeRange, kinds: &[Kind]) -> Result<Fetch>;
}

/// What a source returned for a window.
#[derive(Debug, Clone, Default)]
pub struct Fetch {
    pub items: Vec<Item>,
    /// Conditions that leave the result usable but possibly incomplete, such
    /// as an upstream result cap, worded for the person reading the report.
    pub warnings: Vec<String>,
}

/// Outcome of querying one source.
pub struct Fetched {
    pub source: SourceId,
    pub result: Result<Fetch>,
}

/// Queries every source concurrently.
///
/// A failing source yields an error in its own [`Fetched`] rather than
/// aborting the run, so one unreachable upstream degrades the report to a
/// warning instead of losing the sources that did answer.
pub async fn fetch_all(
    sources: &[Box<dyn Source>],
    range: &TimeRange,
    kinds_for: &dyn Fn(&dyn Source) -> Vec<Kind>,
) -> Vec<Fetched> {
    let futures = sources.iter().map(|source| {
        let id = source.id();
        let kinds = kinds_for(source.as_ref());
        async move {
            let result = source.fetch(range, &kinds).await;
            Fetched { source: id, result }
        }
    });
    future::join_all(futures).await
}
