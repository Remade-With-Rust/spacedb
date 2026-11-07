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

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use redb::{Database, ReadOnlyTable, ReadableTable, Table, TableDefinition, TableError};
use self_cell::self_cell;

use crate::engine::{Durability, KvEngine, ReadTx, Readable, RowVisitor, WriteTx};
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

/// A byte table handle in a read snapshot; `None` records a table absent from it.
type OpenedTable = Option<ReadOnlyTable<&'static [u8], &'static [u8]>>;

/// A redb read transaction (a consistent MVCC snapshot for its lifetime).
pub struct RedbReadTx {
    txn: redb::ReadTransaction,
    /// Table handles opened so far, by name. A `ReadOnlyTable` is independent
    /// of the transaction's borrow, so a snapshot that reads one table a
    /// thousand times resolves it in the table tree once. `None` records a
    /// table that does not exist in this snapshot (it cannot appear later).
    tables: Mutex<HashMap<String, OpenedTable>>,
}

impl RedbReadTx {
    /// Run `f` on the table, opened once per transaction; absent tables yield
    /// `default` (a never-written table reads as empty, not an error -
    /// matching the in-memory engine's "missing table = no keys" semantics).
    fn with_table<R>(
        &self,
        table: &str,
        default: R,
        f: impl FnOnce(&ReadOnlyTable<&'static [u8], &'static [u8]>) -> StoreResult<R>,
    ) -> StoreResult<R> {
        let mut tables = lock(&self.tables);
        if !tables.contains_key(table) {
            let opened = match self.txn.open_table(table_def(table)) {
                Ok(t) => Some(t),
                Err(TableError::TableDoesNotExist(_)) => None,
                Err(e) => return Err(StoreError::engine(e)),
            };
            tables.insert(table.to_string(), opened);
        }
        match tables.get(table).and_then(|t| t.as_ref()) {
            Some(t) => f(t),
            None => Ok(default),
        }
    }
}

impl Readable for RedbReadTx {
    fn get_raw(&self, table: &str, key: &[u8]) -> StoreResult<Option<Vec<u8>>> {
        self.with_table(table, None, |t| {
            let got = t.get(key).map_err(StoreError::engine)?;
            Ok(got.map(|g| g.value().to_vec()))
        })
    }

    fn contains_raw(&self, table: &str, key: &[u8]) -> StoreResult<bool> {
        self.with_table(table, false, |t| {
            let present = t.get(key).map_err(StoreError::engine)?.is_some();
            Ok(present)
        })
    }

    fn range_raw(&self, table: &str, lo: &[u8], hi: &[u8]) -> StoreResult<Vec<(Vec<u8>, Vec<u8>)>> {
        self.with_table(table, Vec::new(), |t| {
            let mut out = Vec::new();
            for entry in t.range(lo..hi).map_err(StoreError::engine)? {
                let (k, v) = entry.map_err(StoreError::engine)?;
                out.push((k.value().to_vec(), v.value().to_vec()));
            }
            Ok(out)
        })
    }

    fn range_with(
        &self,
        table: &str,
        lo: &[u8],
        hi: &[u8],
        f: &mut RowVisitor<'_>,
    ) -> StoreResult<()> {
        self.with_table(table, (), |t| {
            for entry in t.range(lo..hi).map_err(StoreError::engine)? {
                let (k, v) = entry.map_err(StoreError::engine)?;
                f(k.value(), v.value())?;
            }
            Ok(())
        })
    }
}

impl ReadTx for RedbReadTx {}

// ─── write transaction ───────────────────────────────────────────────────────

/// The tables opened in a write transaction, for the transaction's whole life.
type OpenTables<'txn> = Mutex<HashMap<String, Table<'txn, &'static [u8], &'static [u8]>>>;

self_cell! {
    /// A write transaction and the tables open in it, as one value. redb's
    /// `Table<'txn>` borrows the transaction, so the pair is self-referential;
    /// `self_cell` keeps the transaction at a fixed address and drops the
    /// tables before it (each hands its root back to the transaction on drop).
    struct WriteCell {
        owner: redb::WriteTransaction,
        #[not_covariant]
        dependent: OpenTables,
    }
}

/// A redb write transaction. redb buffers mutations and applies them atomically
/// on `commit`; dropping without committing aborts (rollback).
///
/// Every table is opened ONCE per transaction: each put and delete used to
/// open (and on return close) its table, a table-tree lookup and a root
/// handoff per row.
pub struct RedbWriteTx {
    cell: WriteCell,
}

impl RedbWriteTx {
    /// Run `f` on the table, opened on first use. Opening a table in a write
    /// txn creates it if absent, so an empty table simply reads as nothing,
    /// giving read-your-own-writes uniformly.
    fn with_table<R>(
        &self,
        table: &str,
        f: impl for<'t> FnOnce(&mut Table<'t, &'static [u8], &'static [u8]>) -> StoreResult<R>,
    ) -> StoreResult<R> {
        self.cell.with_dependent(|txn, tables| {
            let mut tables = lock(tables);
            if !tables.contains_key(table) {
                let opened = txn.open_table(table_def(table)).map_err(StoreError::engine)?;
                tables.insert(table.to_string(), opened);
            }
            f(tables.get_mut(table).expect("present or just inserted"))
        })
    }
}

impl Readable for RedbWriteTx {
    fn get_raw(&self, table: &str, key: &[u8]) -> StoreResult<Option<Vec<u8>>> {
        self.with_table(table, |t| {
            let got = t.get(key).map_err(StoreError::engine)?;
            Ok(got.map(|g| g.value().to_vec()))
        })
    }

    fn contains_raw(&self, table: &str, key: &[u8]) -> StoreResult<bool> {
        self.with_table(table, |t| {
            let present = t.get(key).map_err(StoreError::engine)?.is_some();
            Ok(present)
        })
    }

    fn range_raw(&self, table: &str, lo: &[u8], hi: &[u8]) -> StoreResult<Vec<(Vec<u8>, Vec<u8>)>> {
        self.with_table(table, |t| {
            let mut out = Vec::new();
            for entry in t.range(lo..hi).map_err(StoreError::engine)? {
                let (k, v) = entry.map_err(StoreError::engine)?;
                out.push((k.value().to_vec(), v.value().to_vec()));
            }
            Ok(out)
        })
    }

    fn range_with(
        &self,
        table: &str,
        lo: &[u8],
        hi: &[u8],
        f: &mut RowVisitor<'_>,
    ) -> StoreResult<()> {
        self.with_table(table, |t| {
            for entry in t.range(lo..hi).map_err(StoreError::engine)? {
                let (k, v) = entry.map_err(StoreError::engine)?;
                f(k.value(), v.value())?;
            }
            Ok(())
        })
    }
}

impl WriteTx for RedbWriteTx {
    fn put_raw(&mut self, table: &str, key: &[u8], val: &[u8]) -> StoreResult<()> {
        self.with_table(table, |t| {
            t.insert(key, val).map_err(StoreError::engine)?;
            Ok(())
        })
    }

    fn delete_raw(&mut self, table: &str, key: &[u8]) -> StoreResult<bool> {
        self.with_table(table, |t| {
            let removed = t.remove(key).map_err(StoreError::engine)?;
            Ok(removed.is_some())
        })
    }

    fn commit(self) -> StoreResult<()> {
        // `into_owner` drops every open table first, then yields the transaction.
        self.cell.into_owner().commit().map_err(StoreError::engine)
    }
}

// ─── engine ──────────────────────────────────────────────────────────────────

impl KvEngine for RedbEngine {
    type RTx<'a> = RedbReadTx;
    type WTx<'a> = RedbWriteTx;

    fn begin_read(&self) -> StoreResult<Self::RTx<'_>> {
        let txn = self.db.begin_read().map_err(StoreError::engine)?;
        Ok(RedbReadTx {
            txn,
            tables: Mutex::new(HashMap::new()),
        })
    }

    fn begin_write(&self, durability: Durability) -> StoreResult<Self::WTx<'_>> {
        let mut txn = self.db.begin_write().map_err(StoreError::engine)?;
        let redb_durability = match durability {
            Durability::Immediate => redb::Durability::Immediate,
            Durability::Eventual => redb::Durability::Eventual,
        };
        txn.set_durability(redb_durability);
        Ok(RedbWriteTx {
            cell: WriteCell::new(txn, |_| Mutex::new(HashMap::new())),
        })
    }
}

/// Lock a transaction's open-table cache. A transaction is used from one thread
/// at a time, so the lock is never contended; it is a `Mutex` rather than a
/// `RefCell` only so the transaction types stay `Sync`. A poisoned cache is
/// still a valid map of open tables.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}
