//! The store error taxonomy.
//!
//! Kept deliberately small in S1 — it covers the engine seam, the two codecs,
//! and table addressing. Later slices extend it additively:
//! - **S2** adds `Crypto` / `Cold` for the AEAD value boundary + the cold-gate.
//! - **S3** adds `Schema` / `SchemaTooNew` for the `_meta` refuse-or-migrate gate.
//!
//! Every variant carries an owned `String` rather than borrowing the underlying
//! engine error type, so `StoreError` stays engine-agnostic (a redb storage
//! error and an in-memory poisoned-lock error both flatten to `Engine`).

use thiserror::Error;

/// The result type returned throughout `spacedb-store`.
pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Debug, Error)]
pub enum StoreError {
    /// A failure originating in the underlying storage engine (redb I/O, a
    /// transaction/commit failure, a poisoned in-memory lock). The string is
    /// the engine's own message, preserved for diagnostics.
    #[error("engine: {0}")]
    Engine(String),

    /// A value failed to (de)serialize through the `postcard` codec. A decode
    /// failure here means stored bytes don't match the caller's value type —
    /// either a schema mismatch or corruption.
    #[error("value codec: {0}")]
    ValueCodec(String),

    /// A key failed to decode from its order-preserving byte encoding. Indicates
    /// a malformed key on disk or a key/type mismatch at a `Table<K, V>` callsite.
    #[error("key decode: {0}")]
    KeyDecode(String),

    /// The vault is locked, so no key material is available for the AEAD value
    /// boundary. Surfaced from [`crate::crypto::CryptoError::Cold`]; callers treat
    /// it as "unlock required", not data loss.
    #[error("vault is cold; unlock required")]
    Cold,

    /// An AEAD operation failed — corrupt ciphertext, a wrong key, or a row/DEK
    /// presented at the wrong location (AAD mismatch). Carries the underlying
    /// [`crate::crypto::CryptoError`] message.
    #[error("crypto: {0}")]
    Crypto(String),

    /// A collection was opened that has no DEK wrapping yet (use the
    /// create-or-open path to provision one).
    #[error("collection not found: {0}")]
    CollectionNotFound(String),

    /// A collection name collides with a reserved table (the `_`-prefixed names
    /// the store uses internally, e.g. `_dek_wrappings`, `_meta`).
    #[error("reserved collection name: {0}")]
    ReservedName(String),

    /// The on-disk store format is **newer** than this software supports. The
    /// store refuses to open rather than risk misreading a future format — the
    /// "never silently open a newer format" rule.
    #[error("store format version {found} is newer than supported {supported}; upgrade the software")]
    SchemaTooNew { found: u32, supported: u32 },

    /// A schema/migration problem — e.g. an older store with no registered
    /// migration to bring it to the current format version.
    #[error("schema: {0}")]
    Schema(String),
}

impl StoreError {
    pub(crate) fn engine(e: impl std::fmt::Display) -> Self {
        StoreError::Engine(e.to_string())
    }

    pub(crate) fn value_codec(e: impl std::fmt::Display) -> Self {
        StoreError::ValueCodec(e.to_string())
    }

    pub(crate) fn key_decode(msg: impl Into<String>) -> Self {
        StoreError::KeyDecode(msg.into())
    }
}
