//! Errors for metering, budgeting, and settlement.

use thiserror::Error;

pub type MeterResult<T> = Result<T, MeterError>;

#[derive(Debug, Error)]
pub enum MeterError {
    /// A priced op exceeded the remaining budget; nothing was charged.
    #[error("over budget: cost {cost} micro-$MATA exceeds remaining {remaining}")]
    OverBudget { cost: u64, remaining: u64 },

    /// A claim failed to (de)serialize.
    #[error("claim codec error: {0}")]
    Codec(String),

    /// A host settlement backend rejected a claim.
    #[error("settlement failed: {0}")]
    Settlement(String),
}
