//! The `_meta` schema gate — **refuse or migrate, never silently open**.
//!
//! redb gives a stable on-disk *page* format; this gate guards the layer above
//! it: SpaceDB's own store format (how `_dek_wrappings`, sealed rows, etc. are
//! laid out). The reserved `_meta` table records a single `store_format_version`,
//! and [`open_meta`] enforces the rule that keeps an appliance upgrade from
//! bricking a user's data:
//!
//! - **Fresh store** → stamp the current version. (`Initialized`)
//! - **Same version** → proceed. (`Current`)
//! - **Older version** → run the registered [`Migration`] steps up to current,
//!   then stamp it. (`Migrated`)
//! - **Newer version** → **refuse** with [`StoreError::SchemaTooNew`]. Reading a
//!   format written by newer software risks silent misinterpretation, so we stop.
//!
//! This is the same discipline as MATA's `dek_wrappings` format gate,
//! generalized to the whole store.
//!
//! Migrations must be **idempotent**: the version is stamped only after all steps
//! succeed, so a crash mid-migration re-runs the steps from the old version on the
//! next open.

use crate::engine::{Durability, KvEngine, WriteTx};
use crate::error::{StoreError, StoreResult};
use crate::table::Table;

/// The reserved metadata table.
pub const META_TABLE: &str = "_meta";

/// The key under which the store format version is recorded in [`META_TABLE`].
pub const STORE_VERSION_KEY: &str = "store_format_version";

/// The store format version this build writes and understands. Bump it (and add
/// a [`Migration`]) whenever the on-disk layout this crate owns changes.
///
/// **v2 (2026-08-28):** collections MAY carry prefixed (optionally
/// zstd-compressed) sealed values, recorded per collection in
/// `_collection_formats` — see [`crate::compress`]. v1 stores migrate by stamp
/// alone ([`PrefixedValuesStamp`]): their existing collections stay legacy-raw
/// and remain readable; only collections created at v2 are prefixed. The bump
/// exists so a v1 binary **refuses** a v2 store ([`crate::StoreError::SchemaTooNew`])
/// instead of postcard-decoding prefixed plaintexts as values — with a
/// non-self-describing codec that mis-decode can silently "succeed", which is
/// the failure this gate exists to prevent.
pub const STORE_FORMAT_VERSION: u32 = 2;

fn meta_table() -> Table<String, u32> {
    Table::new(META_TABLE)
}

/// What [`open_meta`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetaStatus {
    /// A fresh store; the current version was stamped.
    Initialized,
    /// The store was already at the current version.
    Current,
    /// The store was at `from` and was migrated up to the current version.
    Migrated { from: u32 },
}

/// One step in the migration ladder: it upgrades a store at version
/// [`from_version`](Migration::from_version) to `from_version + 1`.
///
/// Object-safe so a ladder is just `&[&dyn Migration<E>]`. Implementations open
/// their own transactions against the engine and **must be idempotent** (see the
/// module note on crash safety).
pub trait Migration<E: KvEngine>: Send + Sync {
    /// The version this step upgrades *from* (producing `from_version + 1`).
    fn from_version(&self) -> u32;

    /// Apply the migration against the engine.
    fn apply(&self, engine: &E) -> StoreResult<()>;
}

/// Read the recorded store format version, or `None` for a never-initialized
/// store.
pub fn read_store_version<E: KvEngine>(engine: &E) -> StoreResult<Option<u32>> {
    let r = engine.begin_read()?;
    meta_table().get(&r, &STORE_VERSION_KEY.to_string())
}

/// Stamp the store format version. **Advanced/admin** — bypasses the gate; normal
/// callers use [`open_meta`]. Useful for tests and recovery tooling.
pub fn write_store_version<E: KvEngine>(engine: &E, version: u32) -> StoreResult<()> {
    let mut w = engine.begin_write(Durability::Immediate)?;
    meta_table().put(&mut w, &STORE_VERSION_KEY.to_string(), &version)?;
    w.commit()
}

/// The v1 → v2 step: a pure stamp. v2 changes only how collections *created at
/// v2* seal their values (prefixed, see [`crate::compress`]); every v1
/// collection has no `_collection_formats` entry and keeps reading and writing
/// legacy-raw rows. Nothing on disk needs rewriting, so applying this step is a
/// no-op and trivially idempotent.
pub struct PrefixedValuesStamp;

impl<E: KvEngine> Migration<E> for PrefixedValuesStamp {
    fn from_version(&self) -> u32 {
        1
    }
    fn apply(&self, _engine: &E) -> StoreResult<()> {
        Ok(())
    }
}

/// Open the store's `_meta` gate at [`STORE_FORMAT_VERSION`], with the built-in
/// ladder (currently just the v1 → v2 stamp).
pub fn open_meta<E: KvEngine>(engine: &E) -> StoreResult<MetaStatus> {
    open_meta_with(engine, STORE_FORMAT_VERSION, &[&PrefixedValuesStamp])
}

/// Open the store's `_meta` gate at `current_version`, running `migrations` to
/// bring an older store up to it. See the module docs for the four cases.
pub fn open_meta_with<E: KvEngine>(
    engine: &E,
    current_version: u32,
    migrations: &[&dyn Migration<E>],
) -> StoreResult<MetaStatus> {
    match read_store_version(engine)? {
        None => {
            write_store_version(engine, current_version)?;
            Ok(MetaStatus::Initialized)
        }
        Some(v) if v == current_version => Ok(MetaStatus::Current),
        Some(v) if v > current_version => Err(StoreError::SchemaTooNew {
            found: v,
            supported: current_version,
        }),
        Some(v) => {
            // v < current_version: walk the ladder one step at a time.
            for from in v..current_version {
                let step = migrations
                    .iter()
                    .find(|m| m.from_version() == from)
                    .ok_or_else(|| {
                        StoreError::Schema(format!(
                            "no migration registered from store format version {from}"
                        ))
                    })?;
                step.apply(engine)?;
            }
            // Stamp current only after every step succeeded (idempotency contract).
            write_store_version(engine, current_version)?;
            Ok(MetaStatus::Migrated { from: v })
        }
    }
}
