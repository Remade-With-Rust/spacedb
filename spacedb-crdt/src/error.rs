//! Errors for the convergent-document layer.

use thiserror::Error;

/// Result alias for `spacedb-crdt`.
pub type CrdtResult<T> = Result<T, CrdtError>;

#[derive(Debug, Error)]
pub enum CrdtError {
    /// A remote update could not be decoded (corrupt or wrong wire version).
    #[error("decode update: {0}")]
    DecodeUpdate(String),

    /// A decoded update failed to apply.
    #[error("apply update: {0}")]
    ApplyUpdate(String),

    /// A peer's state vector could not be decoded.
    #[error("decode state vector: {0}")]
    DecodeStateVector(String),

    /// A register value failed to (de)serialize through JSON.
    #[error("value codec for field `{field}`: {source}")]
    ValueCodec {
        field: String,
        source: serde_json::Error,
    },

    /// An underlying storage error from the persistence layer (S2): the encrypted
    /// `spacedb-store` engine, codec, or AEAD boundary.
    #[error("store: {0}")]
    Store(#[from] spacedb_store::StoreError),
}
