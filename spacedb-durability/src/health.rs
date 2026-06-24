//! Replica-count health — the honest durability state of a snapshot.
//!
//! A snapshot's durability is "how many of its shards are still reachable." This
//! is the signal the repair loop ([`crate::repair`]) acts on, and the honest
//! state an operator surfaces: full redundancy, degraded-but-recoverable, or
//! lost.

use crate::erasure::Manifest;
use crate::error::DurabilityResult;
use crate::fleet::Fleet;
use crate::placement::Placement;

/// How many of a snapshot's shards are currently reachable — placed on an online
/// home that actually holds the bytes.
pub fn reachable_shard_count(
    manifest: &Manifest,
    placement: &Placement,
    fleet: &Fleet,
) -> DurabilityResult<usize> {
    let mut reachable = 0;
    for shard_ref in &manifest.shards {
        let target = match placement.target_for(shard_ref.index) {
            Ok(t) => t,
            Err(_) => continue,
        };
        if let Some(node) = fleet.node(target) {
            if node.is_online() && node.store().has(&shard_ref.hash)? {
                reachable += 1;
            }
        }
    }
    Ok(reachable)
}

/// How many **surplus** copies of a snapshot's shards exist — bytes held by an
/// online home that the placement does not reference (e.g. a repaired-away shard
/// whose original home rejoined). These are reclaimable cost, orthogonal to the
/// under-replication [`HealthStatus`]: a fully `Healthy` snapshot can still carry
/// surplus copies worth dropping ([`crate::reclaim`]).
pub fn surplus_shard_count(
    manifest: &Manifest,
    placement: &Placement,
    fleet: &Fleet,
) -> DurabilityResult<usize> {
    let online = fleet.online_targets();
    let mut surplus = 0;
    for shard_ref in &manifest.shards {
        let placed = match placement.target_for(shard_ref.index) {
            Ok(t) => t,
            Err(_) => continue,
        };
        for t in &online {
            if &t.id == placed {
                continue;
            }
            if let Some(node) = fleet.node(&t.id) {
                if node.store().has(&shard_ref.hash)? {
                    surplus += 1;
                }
            }
        }
    }
    Ok(surplus)
}

/// The durability status of a snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HealthStatus {
    /// Full redundancy: every shard is reachable.
    Healthy,
    /// Recoverable, but redundancy is reduced — `missing` shards need re-placing.
    Degraded { missing: usize },
    /// Below the reconstruction threshold: fewer than `k` shards survive, so the
    /// snapshot cannot be rebuilt from shards.
    Lost,
}

/// A snapshot's replica-count health.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplicaHealth {
    /// Total shards `n`.
    pub total_shards: usize,
    /// Shards required to reconstruct `k`.
    pub shards_needed: usize,
    /// Shards currently reachable.
    pub reachable: usize,
    /// The overall status.
    pub status: HealthStatus,
}

impl ReplicaHealth {
    /// How many more shard losses can be tolerated before data loss
    /// (`reachable - k`, floored at 0).
    pub fn slack(&self) -> usize {
        self.reachable.saturating_sub(self.shards_needed)
    }

    /// Whether the snapshot can still be reconstructed (and therefore repaired).
    pub fn is_repairable(&self) -> bool {
        self.reachable >= self.shards_needed
    }
}

/// Assess a snapshot's replica-count health against the current fleet.
pub fn health(
    manifest: &Manifest,
    placement: &Placement,
    fleet: &Fleet,
) -> DurabilityResult<ReplicaHealth> {
    let reachable = reachable_shard_count(manifest, placement, fleet)?;
    let total = manifest.total_shards();
    let needed = manifest.shards_needed();

    let status = if reachable < needed {
        HealthStatus::Lost
    } else if reachable == total {
        HealthStatus::Healthy
    } else {
        HealthStatus::Degraded {
            missing: total - reachable,
        }
    };

    Ok(ReplicaHealth {
        total_shards: total,
        shards_needed: needed,
        reachable,
        status,
    })
}
