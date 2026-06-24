//! In-memory [`KvEngine`] — the test/ephemeral engine.
//!
//! Its entire reason to exist is to back unit tests and any non-durable path, so
//! it must reproduce the **transactional semantics** of the real engine exactly —
//! not just store bytes. In particular a write transaction here **buffers**
//! mutations in an overlay and applies them atomically on commit; dropping it
//! discards the overlay (rollback). A naïve "mutate the map directly" impl would
//! pass functional tests while silently failing the atomicity/rollback tests,
//! defeating the point of having two engines.
//!
//! Concurrency mirrors the single-writer model via an `RwLock`: a write txn holds
//! the write guard for its lifetime (so no reader observes a half-applied txn),
//! and read txns hold a read guard (a consistent snapshot for their lifetime).

use std::collections::BTreeMap;
use std::ops::Bound;
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::engine::{Durability, KvEngine, ReadTx, Readable, WriteTx};
use crate::error::{StoreError, StoreResult};

type Table = BTreeMap<Vec<u8>, Vec<u8>>;
type Tables = BTreeMap<String, Table>;

/// An in-memory, transactional key/value engine. Loses all data on drop.
#[derive(Default)]
pub struct MemEngine {
    tables: RwLock<Tables>,
}

impl MemEngine {
    pub fn new() -> Self {
        Self::default()
    }
}

fn poisoned() -> StoreError {
    StoreError::engine("in-memory lock poisoned")
}

/// Collect the `[lo, hi)` slice of a table into sorted `(key, value)` pairs.
fn range_table(table: &Table, lo: &[u8], hi: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
    table
        .range::<[u8], _>((Bound::Included(lo), Bound::Excluded(hi)))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

// ─── read transaction ────────────────────────────────────────────────────────

/// A consistent read snapshot held for the transaction's lifetime.
pub struct MemReadTx<'a> {
    guard: RwLockReadGuard<'a, Tables>,
}

impl Readable for MemReadTx<'_> {
    fn get_raw(&self, table: &str, key: &[u8]) -> StoreResult<Option<Vec<u8>>> {
        Ok(self.guard.get(table).and_then(|t| t.get(key).cloned()))
    }

    fn range_raw(&self, table: &str, lo: &[u8], hi: &[u8]) -> StoreResult<Vec<(Vec<u8>, Vec<u8>)>> {
        Ok(self
            .guard
            .get(table)
            .map(|t| range_table(t, lo, hi))
            .unwrap_or_default())
    }
}

impl ReadTx for MemReadTx<'_> {}

// ─── write transaction ───────────────────────────────────────────────────────

/// `Some(bytes)` = put this value; `None` = delete this key. Buffered until commit.
type Overlay = BTreeMap<String, BTreeMap<Vec<u8>, Option<Vec<u8>>>>;

/// A single-writer transaction. Holds the write guard (single-writer) and buffers
/// mutations in `overlay`; `commit` applies them to the base map, `drop` discards.
pub struct MemWriteTx<'a> {
    base: RwLockWriteGuard<'a, Tables>,
    overlay: Overlay,
    _durability: Durability,
}

impl Readable for MemWriteTx<'_> {
    fn get_raw(&self, table: &str, key: &[u8]) -> StoreResult<Option<Vec<u8>>> {
        // Read-your-own-writes: the overlay shadows the base map.
        if let Some(t) = self.overlay.get(table) {
            if let Some(slot) = t.get(key) {
                return Ok(slot.clone());
            }
        }
        Ok(self.base.get(table).and_then(|t| t.get(key).cloned()))
    }

    fn range_raw(&self, table: &str, lo: &[u8], hi: &[u8]) -> StoreResult<Vec<(Vec<u8>, Vec<u8>)>> {
        // Materialize the base slice, then apply this txn's overlay edits within
        // the range so a range scan also sees uncommitted writes.
        let mut merged: BTreeMap<Vec<u8>, Vec<u8>> = self
            .base
            .get(table)
            .map(|t| range_table(t, lo, hi).into_iter().collect())
            .unwrap_or_default();
        if let Some(t) = self.overlay.get(table) {
            for (k, slot) in t.range::<[u8], _>((Bound::Included(lo), Bound::Excluded(hi))) {
                match slot {
                    Some(v) => {
                        merged.insert(k.clone(), v.clone());
                    }
                    None => {
                        merged.remove(k);
                    }
                }
            }
        }
        Ok(merged.into_iter().collect())
    }
}

impl WriteTx for MemWriteTx<'_> {
    fn put_raw(&mut self, table: &str, key: &[u8], val: &[u8]) -> StoreResult<()> {
        self.overlay
            .entry(table.to_string())
            .or_default()
            .insert(key.to_vec(), Some(val.to_vec()));
        Ok(())
    }

    fn delete_raw(&mut self, table: &str, key: &[u8]) -> StoreResult<bool> {
        let existed = self.get_raw(table, key)?.is_some();
        self.overlay
            .entry(table.to_string())
            .or_default()
            .insert(key.to_vec(), None);
        Ok(existed)
    }

    fn commit(mut self) -> StoreResult<()> {
        // Apply atomically under the held write guard. Taking `overlay` by value
        // makes the post-commit state explicit and avoids cloning.
        let overlay = std::mem::take(&mut self.overlay);
        for (table, edits) in overlay {
            let t = self.base.entry(table).or_default();
            for (key, slot) in edits {
                match slot {
                    Some(v) => {
                        t.insert(key, v);
                    }
                    None => {
                        t.remove(&key);
                    }
                }
            }
        }
        Ok(())
    }
}

// ─── engine ──────────────────────────────────────────────────────────────────

impl KvEngine for MemEngine {
    type RTx<'a> = MemReadTx<'a>;
    type WTx<'a> = MemWriteTx<'a>;

    fn begin_read(&self) -> StoreResult<Self::RTx<'_>> {
        let guard = self.tables.read().map_err(|_| poisoned())?;
        Ok(MemReadTx { guard })
    }

    fn begin_write(&self, durability: Durability) -> StoreResult<Self::WTx<'_>> {
        let base = self.tables.write().map_err(|_| poisoned())?;
        Ok(MemWriteTx {
            base,
            overlay: Overlay::new(),
            _durability: durability,
        })
    }
}
