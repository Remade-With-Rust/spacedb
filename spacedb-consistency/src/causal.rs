//! The Causal+ (session) tier — read-your-writes and monotonic reads, no consensus.
//!
//! A [`CausalSession`] carries a **causal token**: the state-vector frontier it has
//! observed. A causal read of a local replica is served only if the replica has
//! caught up to that frontier; otherwise it reports [`Outcome::Stale`] rather than
//! silently serving older data. This gives the two session guarantees people
//! actually want — *I see my own writes*, and *my reads never go backwards* —
//! cheaply and partition-tolerantly, built directly on the convergent substrate's
//! state vectors. No cross-node coordination is involved.

use spacedb_crdt::CrdtDoc;

use crate::outcome::Outcome;
use crate::tier::Tier;

/// A causal-consistency session over one or more replicas of a document.
#[derive(Clone, Debug, Default)]
pub struct CausalSession {
    /// The most-advanced frontier this session has observed (empty = none yet).
    token: Vec<u8>,
}

impl CausalSession {
    pub fn new() -> Self {
        Self::default()
    }

    /// The session's causal token (the observed state-vector frontier).
    pub fn token(&self) -> &[u8] {
        &self.token
    }

    /// Record a local write: the session has now observed everything in `doc`,
    /// including the write just made (read-your-writes). Returns [`Outcome::Local`]
    /// — written and offline-durable, propagation is asynchronous.
    pub fn record_write(&mut self, doc: &CrdtDoc) -> Outcome {
        self.token = doc.state_vector();
        Outcome::Local
    }

    /// Attempt a causal read of `doc`. If `doc` has caught up to the session's
    /// frontier, the read is served (`Committed(Causal)`) and the token advances
    /// to `doc`'s (≥) frontier — so the session never regresses (monotonic reads).
    /// If `doc` is behind, the read is honestly [`Outcome::Stale`] and the token
    /// is *not* advanced.
    pub fn read(&mut self, doc: &CrdtDoc) -> Outcome {
        let lag = if self.token.is_empty() {
            0
        } else {
            doc.ops_behind(&self.token).unwrap_or(0)
        };
        if lag == 0 {
            self.token = doc.state_vector();
            Outcome::Committed(Tier::Causal)
        } else {
            Outcome::Stale { lag_ops: lag }
        }
    }
}
