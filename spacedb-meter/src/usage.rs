//! What gets measured — the three resource classes, computed deterministically.
//!
//! SpaceDB only ever produces *amounts*, never prices: the same input gives the
//! same number on every node, so a claim is reproducible and a host can't be
//! over-billed. Pricing is a separate, swappable concern ([`crate::RateCard`]).

use serde::{Deserialize, Serialize};

/// A measured amount of one resource class over a period.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Usage {
    /// Bytes held, integrated over time and **multiplied by replica count** —
    /// erasure/replication is real cost, so it is metered honestly.
    Storage { byte_seconds: u128 },
    /// On-node compute: deterministic `fuel` consumed (M6 `FunctionRun.fuel_used`)
    /// across some number of `invocations`.
    Compute { fuel: u64, invocations: u64 },
    /// Bytes served to a peer — query results and sync.
    Transit { bytes_served: u64 },
}

impl Usage {
    /// Storage held: `bytes × seconds × replica_count`. Three replicas of the same
    /// bytes for the same time cost three times as much, because they are.
    pub fn storage(bytes: u64, seconds: u64, replica_count: u32) -> Usage {
        Usage::Storage {
            byte_seconds: bytes as u128 * seconds as u128 * replica_count as u128,
        }
    }

    /// Compute consumed by `invocations` runs totalling `fuel` units.
    pub fn compute(fuel: u64, invocations: u64) -> Usage {
        Usage::Compute { fuel, invocations }
    }

    /// Transit billed at the **minimum** of what the server claims to have sent and
    /// what the consumer acknowledges receiving — neither side can inflate it. This
    /// is the bilateral-corroboration rule: bill what both agree on.
    pub fn transit(server_claimed: u64, consumer_acked: u64) -> Usage {
        Usage::Transit {
            bytes_served: server_claimed.min(consumer_acked),
        }
    }

    /// The resource class of this amount.
    pub fn resource(&self) -> Resource {
        match self {
            Usage::Storage { .. } => Resource::Storage,
            Usage::Compute { .. } => Resource::Compute,
            Usage::Transit { .. } => Resource::Transit,
        }
    }
}

/// The class of a resource — the settlement bucket.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resource {
    Storage,
    Compute,
    Transit,
}

impl Resource {
    /// A stable tag used in deterministic claim ids.
    pub fn tag(&self) -> &'static str {
        match self {
            Resource::Storage => "storage",
            Resource::Compute => "compute",
            Resource::Transit => "transit",
        }
    }
}

/// What kind of proof backs a claim (mirrors maestro's `ProofKind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProofKind {
    StorageProbe,
    ComputeAttestation,
    TransitReceipt,
}

/// A content-addressed link to the artifact that backs a claim — e.g. a
/// `FunctionRun` digest (M6) or a proof-of-storage probe response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofRef {
    pub kind: ProofKind,
    /// BLAKE3 digest of the proof artifact.
    pub digest: [u8; 32],
    /// Unix seconds.
    pub at: u64,
}
