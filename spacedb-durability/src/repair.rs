//! The repair loop — restore a snapshot's replica count after homes die.
//!
//! When homes fail, a snapshot's reachable shard count drops. As long as at least
//! `k` shards survive, the lost shards can be regenerated and re-placed:
//!
//! 1. **Classify** every shard as surviving or missing.
//! 2. **Reconstruct** the snapshot from the survivors, then **deterministically
//!    re-encode** it — Reed-Solomon is deterministic, so the regenerated shards
//!    are byte-identical to the originals and verify against the unchanged
//!    manifest.
//! 3. **Re-place** each missing shard onto a fresh online home, preferring the
//!    least-loaded failure domain (so anti-affinity is preserved), and return the
//!    updated placement.
//!
//! This is the "node death → re-place → no human" mechanism; MATA's Falcon drives
//! it (calls [`repair`] when [`crate::health`] reports degraded), but the loop
//! logic is open-core. The manifest is unchanged by repair — only the placement
//! moves.

use std::collections::{HashMap, HashSet};

use crate::distribute::recover;
use crate::erasure::{encode_snapshot, Manifest};
use crate::error::{DurabilityError, DurabilityResult};
use crate::fleet::Fleet;
use crate::placement::{Placement, TargetId, TargetInfo};

/// The outcome of a repair pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepairReport {
    /// The shard indices that were regenerated and re-placed.
    pub repaired_shards: Vec<u16>,
    /// The updated placement record (manifest is unchanged).
    pub new_placement: Placement,
}

/// Restore a snapshot's replica count by regenerating its missing shards onto
/// fresh homes. A no-op (empty report) if nothing is missing. Errors with
/// [`DurabilityError::InsufficientShards`] if fewer than `k` shards survive (the
/// snapshot can't be reconstructed to regenerate the rest), or
/// [`DurabilityError::NotEnoughTargets`] if there aren't enough fresh homes.
pub fn repair(
    manifest: &Manifest,
    placement: &Placement,
    fleet: &Fleet,
) -> DurabilityResult<RepairReport> {
    // 1. Classify shards; track which homes/domains already hold a survivor.
    let mut missing: Vec<u16> = Vec::new();
    let mut surviving_homes: HashSet<&TargetId> = HashSet::new();
    let mut domain_load: HashMap<String, usize> = HashMap::new();

    for shard_ref in &manifest.shards {
        let target = placement.target_for(shard_ref.index)?;
        let node = fleet.node(target);
        let reachable = match node {
            Some(n) => n.is_online() && n.store().has(&shard_ref.hash)?,
            None => false,
        };
        if reachable {
            surviving_homes.insert(target);
            if let Some(n) = node {
                *domain_load.entry(n.domain.clone()).or_default() += 1;
            }
        } else {
            missing.push(shard_ref.index);
        }
    }

    if missing.is_empty() {
        return Ok(RepairReport {
            repaired_shards: Vec::new(),
            new_placement: placement.clone(),
        });
    }

    let total = manifest.total_shards();
    let needed = manifest.shards_needed();
    let reachable = total - missing.len();
    if reachable < needed {
        // Can't reconstruct, so can't regenerate the lost shards.
        return Err(DurabilityError::InsufficientShards {
            have: reachable,
            need: needed,
        });
    }

    // 2. Fresh homes: online, and not already holding a surviving shard.
    let mut fresh: Vec<TargetInfo> = fleet
        .online_targets()
        .into_iter()
        .filter(|t| !surviving_homes.contains(&t.id))
        .collect();
    if fresh.len() < missing.len() {
        return Err(DurabilityError::NotEnoughTargets {
            have: fresh.len(),
            need: missing.len(),
        });
    }

    // 3. Reconstruct + deterministically re-encode to regenerate every shard.
    let snapshot = recover(manifest, placement, fleet)?;
    let (regen_manifest, mut all_shards) =
        encode_snapshot(&snapshot, needed, manifest.parity_shards as usize)?;

    // 4. Re-place each missing shard on a fresh home in the least-loaded domain.
    let mut new_targets = placement.targets().to_vec();
    let mut repaired = Vec::with_capacity(missing.len());
    for &index in &missing {
        let pick = fresh
            .iter()
            .enumerate()
            .min_by_key(|(_, t)| domain_load.get(&t.domain).copied().unwrap_or(0))
            .map(|(i, _)| i)
            .ok_or_else(|| DurabilityError::Placement("no fresh target for repair".into()))?;
        let chosen = fresh.remove(pick);

        let expected = &manifest.shards[index as usize];
        // Deterministic re-encode must reproduce the original shard byte-for-byte.
        // `encode_snapshot` already hashed every shard it produced; compare that
        // rather than hashing the shard a second time.
        if regen_manifest.shards[index as usize].hash != expected.hash {
            return Err(DurabilityError::Erasure(format!(
                "re-encoded shard {index} does not match the manifest"
            )));
        }

        let node = fleet
            .node(&chosen.id)
            .ok_or_else(|| DurabilityError::UnknownTarget(chosen.id.0.clone()))?;
        // Each missing index appears once, so its regenerated shard is moved
        // into the store rather than copied (the rest are dropped with the set).
        let regenerated = std::mem::take(&mut all_shards[index as usize].bytes);
        node.store().put_owned(&expected.hash, regenerated)?;

        new_targets[index as usize] = chosen.id;
        *domain_load.entry(chosen.domain).or_default() += 1;
        repaired.push(index);
    }

    Ok(RepairReport {
        repaired_shards: repaired,
        new_placement: Placement::from_targets(new_targets),
    })
}
