//! SDK errors — the things that stop an op before it runs. Note that a *stale*
//! read or an *unavailable* strong write are not errors: they are honest
//! [`spacedb_consistency::Outcome`]s the op returns. Errors are for "you may not"
//! and "you can't afford it" and "that's not how this field is shaped".

use spacedb_access::DenyReason;
use thiserror::Error;

use crate::schema::CrdtType;

pub type SdkResult<T> = Result<T, SdkError>;

#[derive(Debug, Error)]
pub enum SdkError {
    /// mID authorization refused the op.
    #[error("access denied: {0:?}")]
    Denied(DenyReason),

    /// The agent's budget can't cover the op; nothing was charged or written.
    #[error("over budget: op costs {cost} micro-$MATA, {remaining} remaining")]
    OverBudget { cost: u64, remaining: u64 },

    /// The op doesn't match the field's CRDT type (e.g. incrementing a register).
    #[error("field '{field}' is a {found}, not a {expected}")]
    WrongType {
        field: String,
        expected: CrdtType,
        found: CrdtType,
    },

    /// A strong-tier field must be written via `claim_unique`, not a plain put.
    #[error("field '{0}' is strong-tier; use claim_unique")]
    StrongFieldNeedsClaim(String),

    /// No schema is registered for this collection.
    #[error("unknown collection '{0}'")]
    UnknownCollection(String),

    /// The collection has no such field in its schema.
    #[error("unknown field '{field}' in collection '{collection}'")]
    UnknownField { collection: String, field: String },

    /// An underlying CRDT operation failed.
    #[error("crdt error: {0}")]
    Crdt(String),

    /// An underlying authorization machinery error (not a plain deny).
    #[error("auth error: {0}")]
    Auth(String),
}

impl std::fmt::Display for CrdtType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}
