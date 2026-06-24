//! What the console observes — the raw state an operator's adapters feed in.
//!
//! These are deliberately plain DTOs, not the live types of each SpaceDB layer:
//! the console is a *read-model*, fed by an adapter that translates fleet /
//! access / meter state into observations. That keeps it decoupled and lets the
//! whole dashboard be computed (and tested) from a snapshot.

use serde::{Deserialize, Serialize};
use spacedb_meter::Resource;

/// A home computer in the fleet.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HomeObs {
    pub id: String,
    pub region: String,
    pub online: bool,
}

/// A stored shard and its replication standing.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShardObs {
    pub id: String,
    pub collection: String,
    /// Replicas currently reachable.
    pub reachable_replicas: u32,
    /// The replication target.
    pub target_replicas: u32,
    /// Minimum copies needed to not lose the data (1 for replication, k for k-of-n
    /// erasure coding).
    pub durable_floor: u32,
    pub size_bytes: u64,
}

impl ShardObs {
    /// Below the data already cannot be reconstructed.
    pub fn lost(&self) -> bool {
        self.reachable_replicas < self.durable_floor
    }

    /// Exactly at the floor — a single further loss means data loss.
    pub fn at_risk(&self) -> bool {
        self.reachable_replicas == self.durable_floor
    }

    /// Below the desired redundancy (but maybe still safe).
    pub fn under_replicated(&self) -> bool {
        self.reachable_replicas < self.target_replicas
    }

    /// Above the target — surplus copies that are reclaimable cost, not danger
    /// (e.g. a repaired-away shard whose original home rejoined).
    pub fn over_replicated(&self) -> bool {
        self.reachable_replicas > self.target_replicas
    }

    /// How many replicas above target.
    pub fn excess(&self) -> u32 {
        self.reachable_replicas.saturating_sub(self.target_replicas)
    }
}

/// A strong-tier collection's quorum standing.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StrongObs {
    pub collection: String,
    pub members_online: u32,
    pub members_total: u32,
}

impl StrongObs {
    /// Whether a majority is reachable — i.e. it can still serve linearizable ops.
    pub fn has_quorum(&self) -> bool {
        self.members_online > self.members_total / 2
    }
}

/// A collection's convergence lag on some replica.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LagObs {
    pub collection: String,
    pub lag_ops: u64,
    pub region: Option<String>,
}

/// An issued capability (human or AI agent).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CapabilityObs {
    pub bearer: String,
    pub scope: String,
    pub ops: String,
    pub expiry: Option<u64>,
    pub budget_micro_mata: Option<u64>,
    pub revoked: bool,
}

impl CapabilityObs {
    /// An AI agent mID, by convention `did:agent:*`.
    pub fn is_agent(&self) -> bool {
        self.bearer.starts_with("did:agent:")
    }

    pub fn expires_within(&self, now: u64, window: u64) -> bool {
        matches!(self.expiry, Some(e) if e >= now && e - now <= window)
    }
}

/// An audit-log entry — who did what, and whether it was allowed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditObs {
    pub actor: String,
    pub action: String,
    pub at: u64,
    pub allowed: bool,
}

impl AuditObs {
    pub fn is_agent(&self) -> bool {
        self.actor.starts_with("did:agent:")
    }
}

/// A settled usage receipt — revenue credited to a hosting home for a customer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SettledObs {
    pub host_did: String,
    pub settles_to_did: String,
    pub resource: Resource,
    pub micro_mata: u64,
}

/// An agent's remaining spend against its granted budget.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentBudgetObs {
    pub agent: String,
    pub remaining: u64,
    pub limit: u64,
}
