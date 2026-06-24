//! The shard-store seam: where a shard's bytes live on a host.
//!
//! A `ShardStore` is one home's **content-addressed** blob store — shards are
//! keyed by their BLAKE3 hash, so the key *is* the integrity check. This crate
//! ships [`MemShardStore`] for tests and single-machine use; MATA implements the
//! seam over its `maestro-disco` chunk store + iroh-blobs. A host stores opaque
//! ciphertext fragments it cannot read.

use std::collections::HashMap;
use std::sync::RwLock;

use crate::error::{DurabilityError, DurabilityResult};

/// One host's content-addressed shard storage.
pub trait ShardStore: Send + Sync {
    /// Store `bytes` under their content address `hash`. Idempotent: storing the
    /// same hash twice is a no-op overwrite with identical bytes.
    fn put(&self, hash: &[u8; 32], bytes: &[u8]) -> DurabilityResult<()>;

    /// Fetch the bytes stored under `hash`, or `None` if absent.
    fn get(&self, hash: &[u8; 32]) -> DurabilityResult<Option<Vec<u8>>>;

    /// Whether `hash` is present. Cheaper than `get` for reachability checks.
    fn has(&self, hash: &[u8; 32]) -> DurabilityResult<bool> {
        Ok(self.get(hash)?.is_some())
    }

    /// Remove `hash` (used by repair / GC). Absent keys are a no-op.
    fn delete(&self, hash: &[u8; 32]) -> DurabilityResult<()>;
}

/// In-memory content-addressed shard store. For tests and single-machine use;
/// loses everything on drop.
#[derive(Default)]
pub struct MemShardStore {
    blobs: RwLock<HashMap<[u8; 32], Vec<u8>>>,
}

impl MemShardStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of shards currently held.
    pub fn len(&self) -> usize {
        self.blobs.read().map(|b| b.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn poisoned() -> DurabilityError {
    DurabilityError::Store("in-memory lock poisoned".into())
}

impl ShardStore for MemShardStore {
    fn put(&self, hash: &[u8; 32], bytes: &[u8]) -> DurabilityResult<()> {
        self.blobs.write().map_err(|_| poisoned())?.insert(*hash, bytes.to_vec());
        Ok(())
    }

    fn get(&self, hash: &[u8; 32]) -> DurabilityResult<Option<Vec<u8>>> {
        Ok(self.blobs.read().map_err(|_| poisoned())?.get(hash).cloned())
    }

    fn has(&self, hash: &[u8; 32]) -> DurabilityResult<bool> {
        Ok(self.blobs.read().map_err(|_| poisoned())?.contains_key(hash))
    }

    fn delete(&self, hash: &[u8; 32]) -> DurabilityResult<()> {
        self.blobs.write().map_err(|_| poisoned())?.remove(hash);
        Ok(())
    }
}
