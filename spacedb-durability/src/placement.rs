//! Placement: which target holds which shard, with **anti-affinity**.
//!
//! Durability is only real if shards don't cluster — if a single failure domain
//! (mesh segment / power / owner) can take more than `parity` shards, that one
//! failure is unrecoverable. [`allocate`] spreads shards across distinct domains
//! round-robin, so a domain holds at most `ceil(n / domains)` shards — losing a
//! whole domain stays survivable as long as that is `≤ parity`.
//!
//! The resulting [`Placement`] is a small, serializable record (the user-owned
//! placement index of ADR 0006). M3-S2 models the failure domain as a single
//! label; multi-dimensional anti-affinity (segment × power × owner) is a
//! refinement for later.

use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};

use crate::error::{DurabilityError, DurabilityResult};

/// Identifier of a target node (a home).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TargetId(pub String);

impl From<&str> for TargetId {
    fn from(s: &str) -> Self {
        TargetId(s.to_string())
    }
}

impl From<String> for TargetId {
    fn from(s: String) -> Self {
        TargetId(s)
    }
}

impl std::fmt::Display for TargetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A candidate target for placement: its id and failure domain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetInfo {
    pub id: TargetId,
    pub domain: String,
}

/// A placement record: which target holds each shard, indexed by shard index.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    targets: Vec<TargetId>,
}

impl Placement {
    /// The target holding shard `shard_index`.
    pub fn target_for(&self, shard_index: u16) -> DurabilityResult<&TargetId> {
        self.targets.get(shard_index as usize).ok_or_else(|| {
            DurabilityError::Placement(format!("no placement for shard {shard_index}"))
        })
    }

    /// Number of placed shards.
    pub fn len(&self) -> usize {
        self.targets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    /// The targets, in shard-index order.
    pub fn targets(&self) -> &[TargetId] {
        &self.targets
    }

    /// Serialize the placement record (postcard) for persistence in the user's
    /// own placement index.
    pub fn encode(&self) -> DurabilityResult<Vec<u8>> {
        postcard::to_allocvec(self).map_err(|e| DurabilityError::Placement(e.to_string()))
    }

    /// Deserialize a placement record (postcard).
    pub fn decode(bytes: &[u8]) -> DurabilityResult<Self> {
        postcard::from_bytes(bytes).map_err(|e| DurabilityError::Placement(e.to_string()))
    }

    /// Build a placement directly from a per-shard target list (used by repair to
    /// produce the updated record).
    pub(crate) fn from_targets(targets: Vec<TargetId>) -> Self {
        Self { targets }
    }
}

/// Assign `num_shards` shards across `targets`, one shard per target, spread
/// across distinct failure domains. Requires at least `num_shards` targets.
pub fn allocate(num_shards: usize, targets: &[TargetInfo]) -> DurabilityResult<Placement> {
    if num_shards == 0 {
        return Err(DurabilityError::Placement("num_shards must be >= 1".into()));
    }
    if targets.len() < num_shards {
        return Err(DurabilityError::NotEnoughTargets {
            have: targets.len(),
            need: num_shards,
        });
    }

    // Group targets by failure domain (BTreeMap keeps this deterministic),
    // preserving input order within a domain.
    let mut by_domain: BTreeMap<String, VecDeque<TargetId>> = BTreeMap::new();
    for t in targets {
        by_domain
            .entry(t.domain.clone())
            .or_default()
            .push_back(t.id.clone());
    }

    // Round-robin one target from each domain per pass, so shards spread across
    // domains as evenly as possible (≤ ceil(n / domains) per domain).
    let mut order: Vec<TargetId> = Vec::with_capacity(num_shards);
    while order.len() < num_shards {
        let mut progressed = false;
        for queue in by_domain.values_mut() {
            if order.len() == num_shards {
                break;
            }
            if let Some(id) = queue.pop_front() {
                order.push(id);
                progressed = true;
            }
        }
        if !progressed {
            break; // every domain exhausted (guarded by the count check above)
        }
    }

    if order.len() < num_shards {
        return Err(DurabilityError::NotEnoughTargets {
            have: order.len(),
            need: num_shards,
        });
    }

    Ok(Placement { targets: order })
}
