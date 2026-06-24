//! redb-backed [`KvEngine`] — the durable engine.
//!
//! redb is the mission's chosen engine: MIT/Apache-2.0 (no BUSL in an open-core
//! product), a stable on-disk format, rigorously crash-tested, and **synchronous**
//! (so there is no async/sync bridge to maintain). redb already gives us exactly
//! the transaction model the seam promises — MVCC snapshot reads, single-writer
//! atomic commits, and abort-on-drop rollback — so this adapter is thin: it maps
//! dynamic `&str` table names to `TableDefinition`s and flattens redb's error
//! types into [`StoreError`].
//!
//! **Operational discipline (mission L0):** keep read transactions short — redb
//! reclaims space only past the oldest live read snapshot. Open the database on a
//! **local disk** only; redb's mmap is unsafe over a network filesystem.

use std::path::Path;

use redb::{Database, ReadableTable, TableDefinition, TableError};

use crate::engine::{Durability, KvEngine, ReadTx, Readable, WriteTx};
use crate::error::{StoreError, StoreResult};

/// Byte-keyed, byte-valued table. Typing happens one layer up in [`crate::Table`].
type ByteTable<'a> = TableDefinition<'a, &'static [u8], &'static [u8]>;

fn table_def(name: &str) -> ByteTable<'_> {
    TableDefinition::new(name)
}

/// A durable, transactional key/value engine backed by a single redb file.
pub struct RedbEngine {
    db: Database,
}

impl RedbEngine {
    /// Open (creating if absent) the redb database at `path`.
    pub fn open(path: impl AsRef<Path>) -> StoreResult<Self> {
        let db = Database::create(path).map_err(StoreError::engine)?;
        Ok(Self { db })
    }

    /// Wrap an already-opened redb database.
    pub fn from_db(db: Database) -> Self {
        Self { db }
    }
}

// ─── read transaction ────────────────────────────────────────────────────────

/// A redb read transaction (a consistent MVCC snapshot for its lifetime).
pub struct RedbReadTx {
    txn: redb::ReadTransaction,
}

impl Readable for RedbReadTx {
    fn get_raw(&self, table: &str, key: &[u8]) -> StoreResult<Option<Vec<u8>>> {
        let t = match self.txn.open_table(table_def(table)) {
            Ok(t) => t,
            // A never-written table reads as empty, not an error — matching the
            // in-memory engine's "missing table = no keys" semantics.
            Err(TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(e) => return Err(StoreError::engine(e)),
        };
        let got = t.get(key).map_err(StoreError::engine)?;
        Ok(got.map(|g| g.value().to_vec()))
    }

    fn range_raw(&self, table: &str, lo: &[u8], hi: &[u8]) -> StoreResult<Vec<(Vec<u8>, Vec<u8>)>> {
        let t = match self.txn.open_table(table_def(table)) {
            Ok(t) => t,
            Err(TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(e) => return Err(StoreError::engine(e)),
        };
        let mut out = Vec::new();
        for entry in t.range(lo..hi).map_err(StoreError::engine)? {
            let (k, v) = entry.map_err(StoreError::engine)?;
            out.push((k.value().to_vec(), v.value().to_vec()));
        }
        Ok(out)
    }
}

impl ReadTx for RedbReadTx {}

// ─── write transaction ───────────────────────────────────────────────────────

/// A redb write transaction. redb buffers mutations and applies them atomically
/// on `commit`; dropping without committing aborts (rollback).
pub struct RedbWriteTx {
    txn: redb::WriteTransaction,
}

impl Readable for RedbWriteTx {
    fn get_raw(&self, table: &str, key: &[u8]) -> StoreResult<Option<Vec<u8>>> {
        // Opening a table in a write txn creates it if absent; an empty table
        // simply reads as `None`, giving read-your-own-writes uniformly.
        let t = self.txn.open_table(table_def(table)).map_err(StoreError::engine)?;
        let got = t.get(key).map_err(StoreError::engine)?;
        Ok(got.map(|g| g.value().to_vec()))
    }

    fn range_raw(&self, table: &str, lo: &[u8], hi: &[u8]) -> StoreResult<Vec<(Vec<u8>, Vec<u8>)>> {
        let t = self.txn.open_table(table_def(table)).map_err(StoreError::engine)?;
        let mut out = Vec::new();
        for entry in t.range(lo..hi).map_err(StoreError::engine)? {
            let (k, v) = entry.map_err(StoreError::engine)?;
            out.push((k.value().to_vec(), v.value().to_vec()));
        }
        Ok(out)
    }
}

impl WriteTx for RedbWriteTx {
    fn put_raw(&mut self, table: &str, key: &[u8], val: &[u8]) -> StoreResult<()> {
        let mut t = self.txn.open_table(table_def(table)).map_err(StoreError::engine)?;
        t.insert(key, val).map_err(StoreError::engine)?;
        Ok(())
    }

    fn delete_raw(&mut self, table: &str, key: &[u8]) -> StoreResult<bool> {
        let mut t = self.txn.open_table(table_def(table)).map_err(StoreError::engine)?;
        let removed = t.remove(key).map_err(StoreError::engine)?;
        Ok(removed.is_some())
    }

    fn commit(self) -> StoreResult<()> {
        self.txn.commit().map_err(StoreError::engine)
    }
}

// ─── engine ──────────────────────────────────────────────────────────────────

impl KvEngine for RedbEngine {
    type RTx<'a> = RedbReadTx;
    type WTx<'a> = RedbWriteTx;

    fn begin_read(&self) -> StoreResult<Self::RTx<'_>> {
        let txn = self.db.begin_read().map_err(StoreError::engine)?;
        Ok(RedbReadTx { txn })
    }

    fn begin_write(&self, durability: Durability) -> StoreResult<Self::WTx<'_>> {
        let mut txn = self.db.begin_write().map_err(StoreError::engine)?;
        let redb_durability = match durability {
            Durability::Immediate => redb::Durability::Immediate,
            Durability::Eventual => redb::Durability::Eventual,
        };
        txn.set_durability(redb_durability);
        Ok(RedbWriteTx { txn })
    }
}
