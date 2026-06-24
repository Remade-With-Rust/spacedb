//! The spend cap — DID-is-wallet, and an agent pays from its own budget.
//!
//! An agent-mID is granted a [`Budget`] (the `budget_micro_mata` carried by its M5
//! capability). Every metered, priced op is [`charge`](Budget::charge)d against it;
//! once exhausted the agent is refused, not silently allowed to overspend. This is
//! the runaway-AI backstop in money terms.

use crate::error::MeterError;

/// A remaining spend allowance, in micro-`$MATA`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    remaining: u64,
}

impl Budget {
    pub fn new(micro_mata: u64) -> Self {
        Self {
            remaining: micro_mata,
        }
    }

    pub fn remaining(&self) -> u64 {
        self.remaining
    }

    /// Whether `cost` would fit without overspending.
    pub fn can_afford(&self, cost: u64) -> bool {
        cost <= self.remaining
    }

    /// Charge `cost`, deducting it. Fails with [`MeterError::OverBudget`] and
    /// deducts nothing if the budget can't cover it.
    pub fn charge(&mut self, cost: u64) -> Result<(), MeterError> {
        if cost > self.remaining {
            return Err(MeterError::OverBudget {
                cost,
                remaining: self.remaining,
            });
        }
        self.remaining -= cost;
        Ok(())
    }

    /// Add allowance (a top-up).
    pub fn credit(&mut self, micro_mata: u64) {
        self.remaining = self.remaining.saturating_add(micro_mata);
    }
}
