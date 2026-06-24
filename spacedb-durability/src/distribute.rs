//! Distribute shards onto the fleet, and recover a snapshot from the survivors.
//!
//! These tie the S1 erasure core (`Manifest` + `Shard` + `reconstruct_snapshot`)
//! to the placement and shard-store seams: [`distribute`] writes each shard to
//! its placed home, and [`recover`] fetches whatever the currently-reachable
//! homes still hold and reconstructs — succeeding as long as at least `k` shards
//! survive (so up to `parity` homes can be down).

use crate::erasure::{reconstruct_snapshot, Manifest, Shard};
use crate::error::{DurabilityError, DurabilityResult};
use crate::fleet::Fleet;
use crate::placement::Placement;

/// Write each shard to the home it was placed on. Errors if a placed target is
/// unknown or offline (placement should only assign online targets).
pub fn distribute(
    manifest: &Manifest,
    shards: &[Shard],
    placement: &Placement,
    fleet: &Fleet,
) -> DurabilityResult<()> {
    for shard in shards {
        let target = placement.target_for(shard.index)?;
        let node = fleet
            .node(target)
            .ok_or_else(|| DurabilityError::UnknownTarget(target.0.clone()))?;
        if !node.is_online() {
            return Err(DurabilityError::TargetOffline(target.0.clone()));
        }
        let shard_ref = manifest
            .shards
            .iter()
            .find(|r| r.index == shard.index)
            .ok_or_else(|| {
                DurabilityError::Manifest(format!("no ref for shard {}", shard.index))
            })?;
        node.store().put(&shard_ref.hash, &shard.bytes)?;
    }
    Ok(())
}

/// Reconstruct the snapshot from the shards currently held by online homes.
/// Tolerates up to `parity` missing homes; errors with
/// [`DurabilityError::InsufficientShards`] if fewer than `k` shards survive.
pub fn recover(
    manifest: &Manifest,
    placement: &Placement,
    fleet: &Fleet,
) -> DurabilityResult<Vec<u8>> {
    let mut available = Vec::new();
    for shard_ref in &manifest.shards {
        let target = match placement.target_for(shard_ref.index) {
            Ok(t) => t,
            Err(_) => continue,
        };
        if let Some(node) = fleet.node(target) {
            if node.is_online() {
                if let Some(bytes) = node.store().get(&shard_ref.hash)? {
                    available.push(Shard {
                        index: shard_ref.index,
                        bytes,
                    });
                }
            }
        }
    }
    reconstruct_snapshot(manifest, &available)
}
