//! Reclamation — the counterpart to [`repair`](crate::repair).
//!
//! Repair restores redundancy *up* when homes die; reclamation brings it back
//! *down* afterward. The problem it solves: a home holding shard `i` goes offline,
//! repair regenerates `i` onto a fresh home and re-points the placement there — but
//! the original bytes still sit in the offline home's store. When that home rejoins
//! it is now holding a **surplus copy** the placement no longer references. Since
//! storage is metered per replica, that orphan is pure cost.
//!
//! [`reclaim`] sweeps those orphans. The one safety rule: an orphan is dropped only
//! when the shard's *live* (placed) copy is currently reachable. If the placed copy
//! is down, the orphan is kept — not because [`recover`](crate::recover) needs it
//! (it only reads placed targets) but because it is a free **re-adoption**
//! shortcut: a future repair can re-point the placement at the orphan instead of
//! regenerating the shard from scratch. Deleting it would throw that away.
//!
//! Like repair, this is pure open-core logic; the host's scheduler (MATA's Falcon)
//! decides *when* to run it — ideally after a rejoined home has proven stable, so a
//! flapping home doesn't churn repair/reclaim back and forth.

use crate::erasure::Manifest;
use crate::error::DurabilityResult;
use crate::fleet::Fleet;
use crate::placement::{Placement, TargetId};

/// One surplus copy that was dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReclaimedCopy {
    pub target: TargetId,
    pub shard_index: u16,
}

/// The outcome of a reclamation pass.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReclaimReport {
    /// The surplus copies deleted, in a deterministic order.
    pub reclaimed: Vec<ReclaimedCopy>,
    /// Bytes freed across the fleet.
    pub bytes_reclaimed: u64,
}

impl ReclaimReport {
    pub fn is_empty(&self) -> bool {
        self.reclaimed.is_empty()
    }
}

/// Drop surplus copies of a snapshot's shards — copies held by online homes that
/// the `placement` no longer references — but only for shards whose live placed
/// copy is currently reachable. A no-op (empty report) when there is no safe
/// surplus.
pub fn reclaim(
    manifest: &Manifest,
    placement: &Placement,
    fleet: &Fleet,
) -> DurabilityResult<ReclaimReport> {
    let online = fleet.online_targets();
    let mut report = ReclaimReport::default();

    for shard_ref in &manifest.shards {
        let placed = placement.target_for(shard_ref.index)?;

        // Only reclaim if the live placed copy is reachable; otherwise keep the
        // orphan as a re-adoption shortcut for repair.
        let placed_healthy = match fleet.node(placed) {
            Some(n) => n.is_online() && n.store().has(&shard_ref.hash)?,
            None => false,
        };
        if !placed_healthy {
            continue;
        }

        for t in &online {
            if &t.id == placed {
                continue; // the live copy — keep it
            }
            let node = match fleet.node(&t.id) {
                Some(n) => n,
                None => continue,
            };
            if node.store().has(&shard_ref.hash)? {
                let freed = node
                    .store()
                    .get(&shard_ref.hash)?
                    .map(|b| b.len() as u64)
                    .unwrap_or(0);
                node.store().delete(&shard_ref.hash)?;
                report.bytes_reclaimed += freed;
                report.reclaimed.push(ReclaimedCopy {
                    target: t.id.clone(),
                    shard_index: shard_ref.index,
                });
            }
        }
    }

    report
        .reclaimed
        .sort_by(|a, b| a.shard_index.cmp(&b.shard_index).then(a.target.0.cmp(&b.target.0)));
    Ok(report)
}
