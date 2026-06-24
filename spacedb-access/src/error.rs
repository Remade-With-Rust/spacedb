//! Errors for the access layer.
//!
//! Note the split: a **policy** outcome (a capability is out of scope, expired,
//! the signature doesn't verify) is *not* an error — it is a
//! [`Decision::Deny`](crate::Decision) with a reason, because denial is a normal,
//! expected result. `AccessError` is reserved for genuine **system** failures
//! (key generation, a directory backend error, canonicalization).

use thiserror::Error;

pub type AccessResult<T> = Result<T, AccessError>;

#[derive(Debug, Error)]
pub enum AccessError {
    /// Key generation failed (OS randomness unavailable, or an invalid scalar).
    #[error("key generation: {0}")]
    KeyGen(String),

    /// A public key could not be parsed.
    #[error("invalid key: {0}")]
    InvalidKey(String),

    /// A capability could not be canonicalized for signing/verification.
    #[error("canonicalization: {0}")]
    Canonical(String),

    /// The key directory backend failed (not "key absent" — that is a Deny).
    #[error("directory: {0}")]
    Directory(String),
}
