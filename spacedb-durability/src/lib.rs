#![forbid(unsafe_code)]
//! # spacedb-durability — SpaceDB Layer 2 (cold path)
//!
//! Durability that survives any home dying: a dataset is sealed into a
//! content-addressed, Reed-Solomon erasure-coded snapshot whose shards spread
//! across diverse homes, so losing homes is recoverable rather than fatal.
//!
//! **M4-S1 (here)** is the foundation — the erasure math and the verifiable
//! manifest, over opaque (already-encrypted) snapshot bytes:
//! [`encode_snapshot`] → `(Manifest, shards)`, and [`reconstruct_snapshot`] from
//! any `k`-of-`n` survivors with full integrity checking. Higher slices add the
//! placement and shard-store seams (M4-S2) and the repair loop (M4-S3); MATA
//! implements those over `maestro-disco`.
//!
//! Open-core (MIT): no MATA dependency. Erasure operates on ciphertext, so a
//! hosting home stores shards it cannot read.

mod error;
pub use error::{DurabilityError, DurabilityResult};

mod erasure;
pub use erasure::{encode_snapshot, reconstruct_snapshot, Manifest, Shard, ShardRef};

mod shard_store;
pub use shard_store::{MemShardStore, ShardStore};

mod placement;
pub use placement::{allocate, Placement, TargetId, TargetInfo};

mod fleet;
pub use fleet::{Fleet, Node};

mod distribute;
pub use distribute::{distribute, recover};

mod health;
pub use health::{health, reachable_shard_count, surplus_shard_count, HealthStatus, ReplicaHealth};

mod repair;
pub use repair::{repair, RepairReport};

mod reclaim;
pub use reclaim::{reclaim, ReclaimReport, ReclaimedCopy};
