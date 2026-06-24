#![forbid(unsafe_code)]
//! # spacedb-store — SpaceDB Layer 0
//!
//! The per-node storage **primitive**: a typed, transactional, order-preserving
//! key/value store that everything else in SpaceDB rests on. Getting this small
//! and correct is the whole game — every layer above inherits its guarantees.
//!
//! ## What S1 ships
//!
//! - [`KvEngine`] — the engine seam, with two implementations that share
//!   identical transaction semantics: [`RedbEngine`] (durable) and [`MemEngine`]
//!   (in-memory, for tests).
//! - [`codec`] — a deterministic `postcard` value codec and an order-preserving
//!   key codec (`a < b ⟺ encode(a) < encode(b)`), each a verified bijection.
//! - [`Table`] — the typed `Table<K, V>` primitive that applies both codecs once
//!   so layers above never touch raw bytes.
//!
//! ## Guarantees
//!
//! - **Atomic multi-table writes.** One [`WriteTx`] spans many tables and commits
//!   all-or-nothing; dropping it rolls back.
//! - **Logical-order range scans**, courtesy of the order-preserving key codec.
//! - **Single-writer / snapshot reads**, identical across both engines.
//!
//! ## Open-core boundary
//!
//! `spacedb-store` is MIT and depends on **no** MATA crate. MATA-specific
//! capabilities (the vault key, identity, mesh replication, settlement) enter
//! later through *seams this crate defines* — e.g. the `KeyProvider` for the AEAD
//! boundary in S2 — which MATA implements in its proprietary hosted product. The
//! dependency arrow is MATA → SpaceDB, never the reverse.

mod error;
pub use error::{StoreError, StoreResult};

pub mod codec;
pub use codec::{decode_value, encode_value, KeyDecode, KeyEncode};

pub mod engine;
pub use engine::{Durability, KvEngine, ReadTx, Readable, WriteTx};

pub mod mem_engine;
pub use mem_engine::MemEngine;

pub mod redb_engine;
pub use redb_engine::RedbEngine;

pub mod table;
pub use table::Table;

pub mod crypto;
pub use crypto::{
    open_row, rewrap_dek, seal_row, unwrap_dek, wrap_fresh_dek, CryptoError, KeyProvider,
    StaticKeyProvider, WrappedDek, KEY_LEN, NONCE_LEN,
};

pub mod collection;
pub use collection::Collection;

pub mod meta;
pub use meta::{
    open_meta, open_meta_with, read_store_version, write_store_version, MetaStatus, Migration,
    STORE_FORMAT_VERSION,
};

pub mod extern_value;
pub use extern_value::{classify, content_hash, should_externalize, ExternRef, ValuePlacement};
