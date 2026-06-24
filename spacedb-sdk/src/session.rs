//! A session — who is acting, what they're allowed, and what they can spend.
//!
//! A session binds an actor mID, the capability that authorizes its ops, a
//! [`Budget`] (seeded from the capability's own `budget_micro_mata`, so an agent
//! literally spends from its grant), and a [`CausalSession`] that tracks the
//! frontier it has observed for read-your-writes / monotonic reads.

use spacedb_access::{Did, SignedCapability};
use spacedb_consistency::CausalSession;
use spacedb_meter::Budget;

/// An authenticated, budgeted context for operations.
pub struct Session {
    pub(crate) actor: Did,
    pub(crate) capability: SignedCapability,
    pub(crate) budget: Budget,
    pub(crate) causal: CausalSession,
}

impl Session {
    pub(crate) fn from_capability(capability: SignedCapability) -> Self {
        let actor = capability.capability.bearer.clone();
        // An agent spends from the budget its capability carries; no budget means
        // it can't perform any priced op (fails closed).
        let budget = Budget::new(capability.capability.budget_micro_mata.unwrap_or(0));
        Self {
            actor,
            capability,
            budget,
            causal: CausalSession::new(),
        }
    }

    /// The acting mID.
    pub fn actor(&self) -> &Did {
        &self.actor
    }

    /// Remaining spend allowance, in micro-`$MATA`.
    pub fn budget_remaining(&self) -> u64 {
        self.budget.remaining()
    }
}
