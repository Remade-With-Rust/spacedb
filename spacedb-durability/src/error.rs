//! Errors for the durability layer.

use thiserror::Error;

pub type DurabilityResult<T> = Result<T, DurabilityError>;

#[derive(Debug, Error)]
pub enum DurabilityError {
    /// Invalid erasure parameters (e.g. zero data/parity shards, or more than
    /// 256 total) or a structurally invalid shard (wrong length, out-of-range
    /// index).
    #[error("invalid erasure parameters: {0}")]
    InvalidParams(String),

    /// The Reed-Solomon codec failed to encode or reconstruct.
    #[error("erasure coding: {0}")]
    Erasure(String),

    /// Fewer than `need` (= data-shard count `k`) distinct shards were available,
    /// so the snapshot cannot be reconstructed.
    #[error("insufficient shards: have {have}, need {need}")]
    InsufficientShards { have: usize, need: usize },

    /// A provided shard's bytes did not match its hash in the manifest — corrupt
    /// or tampered. Caught before it can poison reconstruction.
    #[error("shard {index} failed its integrity check")]
    ShardHashMismatch { index: u16 },

    /// The reconstructed snapshot did not match the manifest's snapshot hash.
    #[error("reconstructed snapshot failed its integrity check")]
    SnapshotHashMismatch,

    /// The manifest was structurally inconsistent (e.g. missing a shard ref).
    #[error("manifest: {0}")]
    Manifest(String),

    /// Placement was asked to spread more shards than there are targets to hold
    /// them (one shard per target is required for redundancy).
    #[error("not enough targets: have {have}, need {need}")]
    NotEnoughTargets { have: usize, need: usize },

    /// A placement referenced a target that is not in the fleet.
    #[error("unknown target: {0}")]
    UnknownTarget(String),

    /// Distribution targeted a node that is currently offline.
    #[error("target offline: {0}")]
    TargetOffline(String),

    /// A placement-policy or placement-record problem.
    #[error("placement: {0}")]
    Placement(String),

    /// A shard-store backend error.
    #[error("shard store: {0}")]
    Store(String),
}
