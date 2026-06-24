//! The honesty contract — every op reports the consistency it *actually* achieved.
//!
//! SpaceDB never presents stale-as-fresh or partition-blocked-as-committed. An
//! [`Outcome`] tells the caller exactly what happened, so an app can show
//! "saved locally · syncing" vs "saved", or refuse to oversell a seat, truthfully.

use crate::tier::Tier;

/// Why a (strong) op could not be served.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnavailableReason {
    /// The network is partitioned.
    Partition,
    /// A quorum could not be reached (too few members responded).
    QuorumUnreachable,
}

/// The consistency a read or write actually achieved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The op was committed at this tier (a strong commit, or a causal/convergent
    /// read that is up to date with what the session has observed).
    Committed(Tier),
    /// Written locally and offline-durable, but not yet propagated to peers.
    Local,
    /// Served from a replica that is behind the session's frontier by `lag_ops`
    /// operations — honestly stale, not silently so.
    Stale { lag_ops: usize },
    /// A strong op could not be served and was **not** committed divergently.
    Unavailable(UnavailableReason),
}

impl Outcome {
    /// Whether the op committed at some tier.
    pub fn is_committed(&self) -> bool {
        matches!(self, Outcome::Committed(_))
    }

    /// Whether the op was served at all (anything but `Unavailable`).
    pub fn is_available(&self) -> bool {
        !matches!(self, Outcome::Unavailable(_))
    }

    /// The tier committed at, if any.
    pub fn tier(&self) -> Option<Tier> {
        match self {
            Outcome::Committed(tier) => Some(*tier),
            _ => None,
        }
    }
}
