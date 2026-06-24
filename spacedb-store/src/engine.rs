//! The storage **engine seam** — the one trait everything in SpaceDB rests on.
//!
//! SpaceDB never talks to a concrete database. It talks to [`KvEngine`]: an
//! engine-agnostic, transactional key/value interface. This is what lets the
//! engine be a *per-store decision rather than a rewrite* (redb today; a
//! versioned engine later if temporal reads become a product need — Open Q #6 in
//! the mission), and it is the first of the open-core **seams**: `spacedb-store`
//! ships with the [`crate::RedbEngine`] and [`crate::MemEngine`], and any other
//! engine (including a MATA-hosted one) drops in behind the same trait.
//!
//! ## Transaction model
//!
//! - **Reads** see a consistent snapshot for the transaction's lifetime.
//! - **Writes** are **single-writer** and **atomic**: a [`WriteTx`] buffers its
//!   mutations and applies them all-or-nothing on [`WriteTx::commit`]. Dropping a
//!   write transaction **without** committing **rolls back** — this is the
//!   property the document + index + head-pointer multi-table write depends on,
//!   and the property the durability test in S4 will kill a process to verify.
//! - A [`WriteTx`] is also [`Readable`] (read-your-own-writes within the txn).
//!
//! All keys and values at this layer are **opaque bytes**. Typing and encoding
//! live one layer up in [`crate::Table`]; the AEAD value boundary (S2) lives
//! there too, so the engine only ever sees ciphertext.

use crate::error::StoreResult;

/// Durability for a write transaction. Chosen **per write** because the mission's
/// consistency tiers want different guarantees: ledger-grade / strong-tier
/// collections fsync every commit; explicitly-convergent caches may not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Durability {
    /// fsync on commit — a committed write survives a crash/power loss. The
    /// default for everything unless a collection opts down.
    Immediate,
    /// No fsync barrier on commit — faster, but a crash may lose the most recent
    /// commits. Permitted **only** for explicitly-convergent caches that can
    /// recover by re-syncing.
    Eventual,
}

/// A read view over the store. Both [`ReadTx`] and [`WriteTx`] implement it, so
/// [`crate::Table`] read methods accept either (a write txn reads its own
/// uncommitted writes).
pub trait Readable {
    /// Fetch the raw value bytes for `key` in `table`, or `None` if absent.
    fn get_raw(&self, table: &str, key: &[u8]) -> StoreResult<Option<Vec<u8>>>;

    /// Return `(key, value)` byte pairs in the **half-open** range `[lo, hi)`,
    /// in ascending key (byte-lexicographic) order. Because keys are written in
    /// the order-preserving encoding (see [`crate::codec`]), this is logical
    /// key order.
    fn range_raw(&self, table: &str, lo: &[u8], hi: &[u8]) -> StoreResult<Vec<(Vec<u8>, Vec<u8>)>>;
}

/// A read-only transaction: a consistent snapshot for its lifetime.
pub trait ReadTx: Readable {}

/// A single-writer transaction. Mutations are buffered and applied atomically on
/// [`commit`](WriteTx::commit); dropping without committing rolls back.
pub trait WriteTx: Readable {
    /// Insert or overwrite `key` → `val` in `table`.
    fn put_raw(&mut self, table: &str, key: &[u8], val: &[u8]) -> StoreResult<()>;

    /// Remove `key` from `table`. Returns `true` if a value was present.
    fn delete_raw(&mut self, table: &str, key: &[u8]) -> StoreResult<bool>;

    /// Atomically apply every buffered mutation. Consuming `self` makes
    /// "use after commit" a compile error and "drop without commit" the
    /// rollback path.
    fn commit(self) -> StoreResult<()>;
}

/// The storage engine: opens read and write transactions.
///
/// `Send + Sync` so one engine handle can be shared across the components that
/// need it. The GAT lifetimes let an engine hand a transaction a borrow of
/// itself (the in-memory engine holds a lock guard for the txn's lifetime; redb
/// transactions are self-owned, so they simply ignore the lifetime).
pub trait KvEngine: Send + Sync {
    type RTx<'a>: ReadTx
    where
        Self: 'a;
    type WTx<'a>: WriteTx
    where
        Self: 'a;

    /// Begin a read transaction (a consistent snapshot).
    fn begin_read(&self) -> StoreResult<Self::RTx<'_>>;

    /// Begin a single-writer transaction with the given durability.
    fn begin_write(&self, durability: Durability) -> StoreResult<Self::WTx<'_>>;
}
