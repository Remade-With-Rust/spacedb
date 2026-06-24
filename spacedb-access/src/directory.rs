//! The key-directory seam: DID → published verification key.
//!
//! This is the open-core boundary for identity. A verifier needs the issuer's
//! published key to check a capability's signature; how that key is discovered is
//! an operator concern. This crate ships [`MemKeyDirectory`]; MATA implements the
//! seam over its `did:mata` / IAMHUMAN directory (Supabase-hosted DID documents).

use std::collections::HashMap;
use std::sync::RwLock;

use crate::error::{AccessError, AccessResult};
use crate::identity::{Did, Identity};

/// Resolves a [`Did`] to its published SEC1 verification key.
pub trait KeyDirectory {
    /// The published key bytes for `did`, or `None` if the DID is unknown
    /// (an unknown issuer is a [`Deny`](crate::Decision), not an error).
    fn published_key(&self, did: &Did) -> AccessResult<Option<Vec<u8>>>;
}

/// In-memory DID → key directory, for tests and single-machine use.
#[derive(Default)]
pub struct MemKeyDirectory {
    keys: RwLock<HashMap<Did, Vec<u8>>>,
}

impl MemKeyDirectory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Publish an identity's DID → public key binding.
    pub fn publish(&self, identity: &Identity) -> AccessResult<()> {
        self.publish_key(identity.did().clone(), identity.public_key().to_vec())
    }

    /// Publish a raw DID → SEC1 key binding.
    pub fn publish_key(&self, did: Did, public_sec1: Vec<u8>) -> AccessResult<()> {
        self.keys
            .write()
            .map_err(|_| AccessError::Directory("lock poisoned".into()))?
            .insert(did, public_sec1);
        Ok(())
    }
}

impl KeyDirectory for MemKeyDirectory {
    fn published_key(&self, did: &Did) -> AccessResult<Option<Vec<u8>>> {
        Ok(self
            .keys
            .read()
            .map_err(|_| AccessError::Directory("lock poisoned".into()))?
            .get(did)
            .cloned())
    }
}
