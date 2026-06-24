#![forbid(unsafe_code)]
//! # spacedb-crdt — SpaceDB Layer 1 (convergent collections)
//!
//! The default data tier, and the reason SpaceDB survives a partition-prone mesh:
//! data is modeled as **Y-CRDT** (via `yrs`), so every write is locally available
//! and merges conflict-free with no coordination. Writing offline on a
//! Starlink-partitioned home is a non-event, not an error.
//!
//! M2-S1 ships [`CrdtDoc`]: a document with a typed field→CRDT-type mapping
//! (LWW-Register + PN-Counter), local-first mutation, and the sync primitives
//! (state vector, incremental update, merge). The order-independent convergence
//! property — *the same updates in any order produce the same state* — is proven
//! by the fuzzed test in `tests/convergence.rs`.
//!
//! Open-core (MIT): this depends on `yrs` directly, never on a MATA crate.
//! Persistence into the encrypted `spacedb-store` is M2-S2.

mod error;
pub use error::{CrdtError, CrdtResult};

mod doc;
pub use doc::CrdtDoc;

mod reactive;
pub use reactive::{ReactiveQuery, Watcher};

mod persist;
pub use persist::{CrdtStore, CRDT_DOCS_COLLECTION};

mod compact;
pub use compact::compact_updates;
