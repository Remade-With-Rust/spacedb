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

use crate::engine::{Durability, KvEngine, ReadTx, Readable, RowVisitor, WriteTx};
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

/// `range_raw` on top of `range_with`: one copy per row, taken at the visitor.
fn collect_range(
    tx: &impl Readable,
    table: &str,
    lo: &[u8],
    hi: &[u8],
) -> StoreResult<Vec<(Vec<u8>, Vec<u8>)>> {
    let mut out = Vec::new();
    tx.range_with(table, lo, hi, &mut |k, v| {
        out.push((k.to_vec(), v.to_vec()));
        Ok(())
    })?;
    Ok(out)
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

    fn contains_raw(&self, table: &str, key: &[u8]) -> StoreResult<bool> {
        Ok(self.guard.get(table).is_some_and(|t| t.contains_key(key)))
    }

    fn range_raw(&self, table: &str, lo: &[u8], hi: &[u8]) -> StoreResult<Vec<(Vec<u8>, Vec<u8>)>> {
        collect_range(self, table, lo, hi)
    }

    fn range_with(
        &self,
        table: &str,
        lo: &[u8],
        hi: &[u8],
        f: &mut RowVisitor<'_>,
    ) -> StoreResult<()> {
        if let Some(t) = self.guard.get(table) {
            for (k, v) in t.range::<[u8], _>((Bound::Included(lo), Bound::Excluded(hi))) {
                f(k, v)?;
            }
        }
        Ok(())
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

impl MemWriteTx<'_> {
    /// This transaction's edits to `table`, created on the first edit. The
    /// name is borrowed for the lookup: `entry(table.to_string())` copied it
    /// onto the heap for every single put and delete, to be freed again at
    /// once on every hit.
    fn overlay_table(&mut self, table: &str) -> &mut BTreeMap<Vec<u8>, Option<Vec<u8>>> {
        if !self.overlay.contains_key(table) {
            self.overlay.insert(table.to_string(), BTreeMap::new());
        }
        self.overlay.get_mut(table).expect("present or just inserted")
    }
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

    fn contains_raw(&self, table: &str, key: &[u8]) -> StoreResult<bool> {
        if let Some(t) = self.overlay.get(table) {
            if let Some(slot) = t.get(key) {
                return Ok(slot.is_some());
            }
        }
        Ok(self.base.get(table).is_some_and(|t| t.contains_key(key)))
    }

    fn range_raw(&self, table: &str, lo: &[u8], hi: &[u8]) -> StoreResult<Vec<(Vec<u8>, Vec<u8>)>> {
        collect_range(self, table, lo, hi)
    }

    fn range_with(
        &self,
        table: &str,
        lo: &[u8],
        hi: &[u8],
        f: &mut RowVisitor<'_>,
    ) -> StoreResult<()> {
        // The base slice with this txn's overlay edits applied, so a range scan
        // also sees uncommitted writes. Both sides are sorted by key, so one
        // merge pass yields the rows in order; each surviving row is lent to
        // the visitor in place (it used to be copied out, and before that
        // copied into a temporary map and out again).
        let bounds = (Bound::Included(lo), Bound::Excluded(hi));
        let mut base = self
            .base
            .get(table)
            .into_iter()
            .flat_map(|t| t.range::<[u8], _>(bounds))
            .peekable();
        let mut edits = self
            .overlay
            .get(table)
            .into_iter()
            .flat_map(|t| t.range::<[u8], _>(bounds))
            .peekable();
        loop {
            let take_edit = match (base.peek(), edits.peek()) {
                (None, None) => break,
                (Some(_), None) => false,
                (None, Some(_)) => true,
                (Some((bk, _)), Some((ek, _))) => {
                    if ek <= bk {
                        if ek == bk {
                            base.next(); // the edit shadows the base row
                        }
                        true
                    } else {
                        false
                    }
                }
            };
            if take_edit {
                let (k, slot) = edits.next().expect("peeked");
                if let Some(v) = slot {
                    f(k, v)?;
                }
            } else {
                let (k, v) = base.next().expect("peeked");
                f(k, v)?;
            }
        }
        Ok(())
    }
}

impl WriteTx for MemWriteTx<'_> {
    fn put_raw(&mut self, table: &str, key: &[u8], val: &[u8]) -> StoreResult<()> {
        self.overlay_table(table).insert(key.to_vec(), Some(val.to_vec()));
        Ok(())
    }

    fn put_raw_owned(&mut self, table: &str, key: Vec<u8>, mut val: Vec<u8>) -> StoreResult<()> {
        // Moved, not copied. Trimmed to its length, so a row grown in place
        // keeps no more memory than the copy `put_raw` would have made
        // (shrinking is an in-place realloc, not a copy).
        val.shrink_to_fit();
        self.overlay_table(table).insert(key, Some(val));
        Ok(())
    }

    fn delete_raw(&mut self, table: &str, key: &[u8]) -> StoreResult<bool> {
        // Presence only: the old value is not copied to learn that it was there.
        let existed = self.contains_raw(table, key)?;
        self.overlay_table(table).insert(key.to_vec(), None);
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
