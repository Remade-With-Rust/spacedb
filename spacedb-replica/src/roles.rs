//! Replica roles — what a node holds and serves.
//!
//! Not every device is a full replica (Principle: capability-gated). M3 scaffolds
//! the roles so later milestones (mesh durability + placement, M4) can build on
//! them; **partial-replica subset selection is a stub** here because M3 syncs one
//! document at a time — real "which collections/docs does this cache hold, and how
//! does it evict" logic lands with multi-document placement.

use std::collections::BTreeSet;

/// What a replica holds and serves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplicaRole {
    /// Holds the entire dataset; serves reads and on-node compute (a Home
    /// Computer).
    Full,
    /// Holds a working subset (e.g. a phone's LRU window); offline-first for what
    /// it has.
    PartialCache(SubsetSpec),
    /// Holds no replica; queries the nearest full replica (a thin buyer client).
    BuyerOnly,
}

impl ReplicaRole {
    /// Whether this role keeps `doc_id` locally.
    pub fn holds(&self, doc_id: &str) -> bool {
        match self {
            ReplicaRole::Full => true,
            ReplicaRole::BuyerOnly => false,
            ReplicaRole::PartialCache(spec) => spec.holds(doc_id),
        }
    }

    /// Whether this role serves reads locally (a buyer-only client does not).
    pub fn serves_reads(&self) -> bool {
        !matches!(self, ReplicaRole::BuyerOnly)
    }
}

/// Which documents a partial replica keeps. **Stub** for M3: real subset
/// selection (LRU windows, working-set tracking, eviction) is M4 placement work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubsetSpec {
    /// Hold every document (a partial cache that currently mirrors everything).
    All,
    /// Hold exactly this explicit set of document ids.
    DocIds(BTreeSet<String>),
}

impl SubsetSpec {
    /// Whether this subset includes `doc_id`.
    pub fn holds(&self, doc_id: &str) -> bool {
        match self {
            SubsetSpec::All => true,
            SubsetSpec::DocIds(ids) => ids.contains(doc_id),
        }
    }
}
