//! Board 1 — Fleet Health. "Is anything on fire or about to lose data?"

use serde::{Deserialize, Serialize};

use crate::observe::{HomeObs, LagObs, ShardObs, StrongObs};

/// The single-glance rollup.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthStatus {
    /// Everything reachable and durable.
    Green,
    /// Degraded — redundancy or freshness is below target, but nothing is lost.
    Amber,
    /// Data is lost, or a strong-tier collection can no longer serve.
    Red,
}

/// The durability + availability picture for the whole fleet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetHealth {
    pub homes_total: usize,
    pub homes_online: usize,
    pub shards_total: usize,
    pub shards_under_replicated: usize,
    pub shards_at_risk: usize,
    pub shards_lost: usize,
    /// Surplus copies above target — reclaimable cost, not a durability danger.
    pub shards_over_replicated: usize,
    pub strong_collections: usize,
    pub strong_without_quorum: usize,
    pub stale_collections: usize,
    pub worst_lag_ops: u64,
    pub bytes_stored: u64,
    pub status: HealthStatus,
}

/// Assess fleet health from observations. A collection is "stale" if its lag
/// exceeds `lag_ops_warn`.
pub fn assess_fleet(
    homes: &[HomeObs],
    shards: &[ShardObs],
    strong: &[StrongObs],
    lags: &[LagObs],
    lag_ops_warn: u64,
) -> FleetHealth {
    let homes_online = homes.iter().filter(|h| h.online).count();

    let mut under = 0;
    let mut at_risk = 0;
    let mut lost = 0;
    let mut over = 0;
    let mut bytes_stored = 0u64;
    for s in shards {
        bytes_stored += s.size_bytes;
        if s.lost() {
            lost += 1;
        } else if s.at_risk() {
            at_risk += 1;
        } else if s.under_replicated() {
            under += 1;
        }
        // Orthogonal to under-replication: a shard can be over its target while
        // every other shard is healthy.
        if s.over_replicated() {
            over += 1;
        }
    }

    let strong_without_quorum = strong.iter().filter(|s| !s.has_quorum()).count();
    let stale_collections = lags.iter().filter(|l| l.lag_ops > lag_ops_warn).count();
    let worst_lag_ops = lags.iter().map(|l| l.lag_ops).max().unwrap_or(0);

    let status = if lost > 0 || strong_without_quorum > 0 {
        HealthStatus::Red
    } else if under > 0
        || at_risk > 0
        || stale_collections > 0
        || homes_online < homes.len()
    {
        HealthStatus::Amber
    } else {
        HealthStatus::Green
    };

    FleetHealth {
        homes_total: homes.len(),
        homes_online,
        shards_total: shards.len(),
        shards_under_replicated: under,
        shards_at_risk: at_risk,
        shards_lost: lost,
        shards_over_replicated: over,
        strong_collections: strong.len(),
        strong_without_quorum,
        stale_collections,
        worst_lag_ops,
        bytes_stored,
        status,
    }
}
