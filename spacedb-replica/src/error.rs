//! Errors for the replication layer.

use thiserror::Error;

pub type ReplicaResult<T> = Result<T, ReplicaError>;

#[derive(Debug, Error)]
pub enum ReplicaError {
    /// A CRDT operation failed (decode/apply/encode of an update or state vector).
    #[error(transparent)]
    Crdt(#[from] spacedb_crdt::CrdtError),

    /// The transport failed to deliver or receive a frame.
    #[error("transport: {0}")]
    Transport(String),

    /// A received sync frame was malformed (bad tag / truncated).
    #[error("malformed sync frame")]
    MalformedFrame,
}
