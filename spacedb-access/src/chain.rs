//! Delegation chains — bounded re-delegation of a capability.
//!
//! A bearer holding a delegable capability (`delegation_depth > 0`) can issue a
//! **sub-grant** to another bearer, narrower than the one it holds. The presented
//! credential is then a [`CapabilityChain`]: `[root, sub₁, sub₂, …]` where each
//! link is signed by the previous link's bearer (the delegator), and
//! [`authorize_chain`](crate::authorize_chain) enforces that every step narrows
//! (scope ⊆, ops ⊆, expiry ≤, depth −1) and that no link is revoked.
//!
//! Accountability propagates down the chain: the final bearer's authority traces
//! back, link by link, to the owner who signed the root.

use serde::{Deserialize, Serialize};

use crate::capability::{Capability, SignedCapability};
use crate::error::{AccessError, AccessResult};
use crate::identity::{Did, Identity};

/// A root capability plus a sequence of sub-grants, each signed by the previous
/// bearer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityChain {
    links: Vec<SignedCapability>,
}

impl CapabilityChain {
    /// A chain of just the root grant (no delegation).
    pub fn single(root: SignedCapability) -> Self {
        Self { links: vec![root] }
    }

    /// The links, `[root, …, leaf]`.
    pub fn links(&self) -> &[SignedCapability] {
        &self.links
    }

    pub fn len(&self) -> usize {
        self.links.len()
    }

    pub fn is_empty(&self) -> bool {
        self.links.is_empty()
    }

    /// The bearer the chain ultimately authorizes (the leaf bearer).
    pub fn bearer(&self) -> Option<&Did> {
        self.links.last().map(|l| &l.capability.bearer)
    }

    /// Serialize the chain (postcard) for transmission.
    pub fn encode(&self) -> AccessResult<Vec<u8>> {
        postcard::to_allocvec(self).map_err(|e| AccessError::Canonical(e.to_string()))
    }

    /// Deserialize a chain (postcard).
    pub fn decode(bytes: &[u8]) -> AccessResult<Self> {
        postcard::from_bytes(bytes).map_err(|e| AccessError::Canonical(e.to_string()))
    }
}

impl From<SignedCapability> for CapabilityChain {
    fn from(root: SignedCapability) -> Self {
        Self::single(root)
    }
}

/// Extend `parent` by delegating `sub`, signed by `delegator`. The narrowing and
/// depth constraints are enforced at authorization time
/// ([`authorize_chain`](crate::authorize_chain)); this just signs and appends, so
/// callers should build `sub` with `issuer = delegator.did()` and a scope/ops/
/// expiry within the parent.
pub fn delegate(
    parent: &CapabilityChain,
    sub: Capability,
    delegator: &Identity,
) -> AccessResult<CapabilityChain> {
    let signed = SignedCapability::sign(sub, delegator)?;
    let mut links = parent.links.clone();
    links.push(signed);
    Ok(CapabilityChain { links })
}
