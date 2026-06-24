//! Revocation — immediate, fail-closed.
//!
//! A [`RevocationSet`] is the set of revoked capability ids a node currently
//! knows about. [`authorize`](crate::authorize) checks it on every access and (in
//! a chain) on every link, so a revoked grant — or any ancestor of it — is denied
//! the moment the node learns of the revocation.
//!
//! The honest boundary: "immediate" means *as soon as the revocation reaches this
//! node's set*. Propagating revocations across a partitioned mesh is the
//! transport's job; the engine simply fails closed against whatever set it holds.

use std::collections::HashSet;

/// The set of revoked capability ids known to a node.
#[derive(Clone, Debug, Default)]
pub struct RevocationSet {
    revoked: HashSet<[u8; 16]>,
}

impl RevocationSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Revoke a capability by its id. Idempotent.
    pub fn revoke(&mut self, capability_id: [u8; 16]) {
        self.revoked.insert(capability_id);
    }

    /// Whether a capability id has been revoked.
    pub fn is_revoked(&self, capability_id: &[u8; 16]) -> bool {
        self.revoked.contains(capability_id)
    }

    /// Number of revoked ids.
    pub fn len(&self) -> usize {
        self.revoked.len()
    }

    pub fn is_empty(&self) -> bool {
        self.revoked.is_empty()
    }
}
