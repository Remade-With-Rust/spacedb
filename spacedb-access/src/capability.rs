//! The capability — a signed, scoped, expiring grant.
//!
//! An owner mints a [`Capability`] to a bearer (a human or an AI agent),
//! describing exactly what it may do (`scope` × `ops`), for how long (`expiry`),
//! within what budget, and how far it may be re-delegated. The owner signs the
//! canonical bytes, producing a [`SignedCapability`] anyone can verify against the
//! owner's published key. Nothing is accessible without one (for AI), and
//! everything granted is attributable, expiring, and (S2) revocable.

use serde::{Deserialize, Serialize};

use crate::error::{AccessError, AccessResult};
use crate::identity::{Did, Identity};

/// What a capability applies to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scope {
    /// An entire collection (and every document in it).
    Collection(String),
    /// A single document within a collection.
    Document { collection: String, doc_id: String },
    /// A named function (compute).
    Function(String),
}

impl Scope {
    /// Whether this (granted) scope covers a `requested` access scope. A
    /// collection grant covers any document in it; document and function grants
    /// match exactly.
    pub fn covers(&self, requested: &Scope) -> bool {
        match (self, requested) {
            (Scope::Collection(c), Scope::Collection(rc)) => c == rc,
            (Scope::Collection(c), Scope::Document { collection, .. }) => c == collection,
            (
                Scope::Document { collection, doc_id },
                Scope::Document {
                    collection: rc,
                    doc_id: rd,
                },
            ) => collection == rc && doc_id == rd,
            (Scope::Function(f), Scope::Function(rf)) => f == rf,
            _ => false,
        }
    }
}

/// The operations a capability grants — a bitset of read / write / compute.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ops(u8);

impl Ops {
    pub const NONE: Ops = Ops(0);
    pub const READ: Ops = Ops(1);
    pub const WRITE: Ops = Ops(2);
    pub const COMPUTE: Ops = Ops(4);

    /// Whether `self` grants every op in `needed` (and `needed` is non-empty).
    pub fn contains(self, needed: Ops) -> bool {
        needed.0 != 0 && (self.0 & needed.0) == needed.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether `self` is a subset of `other` — used to check a sub-grant doesn't
    /// escalate ops beyond its parent (S2).
    pub fn is_subset_of(self, other: Ops) -> bool {
        (self.0 & other.0) == self.0
    }
}

impl std::ops::BitOr for Ops {
    type Output = Ops;
    fn bitor(self, rhs: Ops) -> Ops {
        Ops(self.0 | rhs.0)
    }
}

/// A grant of access from an issuer to a bearer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capability {
    /// Unique grant id (the revocation key).
    pub id: [u8; 16],
    /// Who granted this (the owner / a delegating bearer).
    pub issuer: Did,
    /// Who may use it (a human or an AI agent).
    pub bearer: Did,
    /// What it applies to.
    pub scope: Scope,
    /// Which operations it allows.
    pub ops: Ops,
    /// Optional expiry (unix seconds); `None` = until revoked.
    pub expiry: Option<u64>,
    /// Optional spend cap in micro-`$MATA` (the metering hook for M8).
    pub budget_micro_mata: Option<u64>,
    /// How many more times this may be re-delegated (0 = not delegable).
    pub delegation_depth: u8,
}

impl Capability {
    /// Mint a fresh capability with a random id, no expiry/budget, non-delegable.
    /// Refine with the builder methods.
    pub fn grant(
        issuer: impl Into<Did>,
        bearer: impl Into<Did>,
        scope: Scope,
        ops: Ops,
    ) -> AccessResult<Self> {
        let mut id = [0u8; 16];
        getrandom::fill(&mut id).map_err(|e| AccessError::KeyGen(e.to_string()))?;
        Ok(Self {
            id,
            issuer: issuer.into(),
            bearer: bearer.into(),
            scope,
            ops,
            expiry: None,
            budget_micro_mata: None,
            delegation_depth: 0,
        })
    }

    pub fn with_expiry(mut self, unix_seconds: u64) -> Self {
        self.expiry = Some(unix_seconds);
        self
    }

    pub fn with_budget(mut self, micro_mata: u64) -> Self {
        self.budget_micro_mata = Some(micro_mata);
        self
    }

    pub fn with_delegation_depth(mut self, depth: u8) -> Self {
        self.delegation_depth = depth;
        self
    }

    /// The canonical bytes that get signed (deterministic via postcard).
    pub(crate) fn canonical_bytes(&self) -> AccessResult<Vec<u8>> {
        postcard::to_allocvec(self).map_err(|e| AccessError::Canonical(e.to_string()))
    }
}

/// A capability plus the issuer's signature over its canonical bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedCapability {
    pub capability: Capability,
    /// DER ECDSA signature by the issuer over `capability.canonical_bytes()`.
    pub issuer_signature: Vec<u8>,
}

impl SignedCapability {
    /// Sign `capability` with `issuer`. The capability's `issuer` field should be
    /// `issuer.did()`; if it isn't, verification will fail (the directory key for
    /// the claimed issuer won't match this signature).
    pub fn sign(capability: Capability, issuer: &Identity) -> AccessResult<Self> {
        let bytes = capability.canonical_bytes()?;
        let issuer_signature = issuer.sign(&bytes);
        Ok(Self {
            capability,
            issuer_signature,
        })
    }

    /// Serialize for transmission/persistence (postcard).
    pub fn encode(&self) -> AccessResult<Vec<u8>> {
        postcard::to_allocvec(self).map_err(|e| AccessError::Canonical(e.to_string()))
    }

    /// Deserialize a signed capability (postcard).
    pub fn decode(bytes: &[u8]) -> AccessResult<Self> {
        postcard::from_bytes(bytes).map_err(|e| AccessError::Canonical(e.to_string()))
    }
}
