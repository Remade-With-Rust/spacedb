//! Reactive queries — re-evaluate as the document changes.
//!
//! A [`CrdtDoc`] bumps a revision counter on every change (local mutation *or*
//! applied remote update). [`Watcher`] observes that counter; [`ReactiveQuery`]
//! turns it into a live query that re-evaluates when the document changes and
//! **emits a result only when the result actually changes** — the "re-render as
//! the mesh converges" behaviour the SDK exposes as a signal/stream.
//!
//! Polling, not callbacks: the yrs update observer fires *inside* the mutating
//! transaction, where opening another transaction (to read fields) would panic.
//! So changes are recorded as a counter bump and the query re-evaluates safely
//! afterwards, when the caller polls.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::CrdtDoc;

/// Watches a document's revision counter for changes.
pub struct Watcher {
    revision: Arc<AtomicU64>,
    last_seen: Cell<u64>,
}

impl Watcher {
    pub(crate) fn new(revision: Arc<AtomicU64>) -> Self {
        let current = revision.load(Ordering::Relaxed);
        Self {
            revision,
            last_seen: Cell::new(current),
        }
    }

    /// Return whether the document has changed since the last call, consuming the
    /// pending change so a subsequent call returns `false` until the next change.
    pub fn drain_changed(&self) -> bool {
        let current = self.revision.load(Ordering::Relaxed);
        if current != self.last_seen.get() {
            self.last_seen.set(current);
            true
        } else {
            false
        }
    }

    /// The current revision of the watched document.
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }
}

/// A live query over a document: re-evaluates `query` when the document changes
/// and yields the new result only when it differs from the last emitted one.
pub struct ReactiveQuery<R, F> {
    watcher: Watcher,
    query: F,
    last: R,
}

impl<R, F> ReactiveQuery<R, F>
where
    R: Clone + PartialEq,
    F: Fn(&CrdtDoc) -> R,
{
    /// Build a reactive query over `doc`, evaluating `query` once for the initial
    /// value.
    pub fn new(doc: &CrdtDoc, query: F) -> Self {
        let watcher = doc.watch();
        let last = query(doc);
        Self {
            watcher,
            query,
            last,
        }
    }

    /// Poll for an update. Returns `Some(new_result)` if the document changed
    /// *and* the query result changed since the last emission (the pushed delta);
    /// otherwise `None`.
    pub fn poll(&mut self, doc: &CrdtDoc) -> Option<R> {
        if !self.watcher.drain_changed() {
            return None;
        }
        let next = (self.query)(doc);
        if next != self.last {
            self.last = next.clone();
            Some(next)
        } else {
            None
        }
    }

    /// The last evaluated result.
    pub fn current(&self) -> &R {
        &self.last
    }
}
