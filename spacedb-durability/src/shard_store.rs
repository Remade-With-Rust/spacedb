//! The shard-store seam: where a shard's bytes live on a host.
//!
//! A `ShardStore` is one home's **content-addressed** blob store — shards are
//! keyed by their BLAKE3 hash, so the key *is* the integrity check. This crate
//! ships [`MemShardStore`] for tests and single-machine use; MATA implements the
//! seam over its `maestro-disco` chunk store + iroh-blobs. A host stores opaque
//! ciphertext fragments it cannot read.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
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

    /// [`put`](Self::put) taking ownership, for a caller done with the bytes:
    /// a store that keeps owned buffers moves them in instead of copying. The
    /// default copies, as `put` does.
    fn put_owned(&self, hash: &[u8; 32], bytes: Vec<u8>) -> DurabilityResult<()> {
        self.put(hash, &bytes)
    }

    /// The length of the bytes stored under `hash`, or `None` if absent.
    /// Cheaper than `get` when only the size matters (reclaim accounting).
    fn len_of(&self, hash: &[u8; 32]) -> DurabilityResult<Option<usize>> {
        Ok(self.get(hash)?.map(|b| b.len()))
    }

    /// Remove `hash` (used by repair / GC). Absent keys are a no-op.
    fn delete(&self, hash: &[u8; 32]) -> DurabilityResult<()>;
}

/// In-memory content-addressed shard store. For tests and single-machine use;
/// loses everything on drop.
#[derive(Default)]
pub struct MemShardStore {
    blobs: RwLock<HashMap<[u8; 32], Vec<u8>, BuildHasherDefault<ContentHashHasher>>>,
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

/// Hashes a BLAKE3 content address by taking 8 of its bytes. A content hash is
/// already uniform and cannot be steered toward collisions without a preimage,
/// so re-hashing it (SipHash, by default) bought nothing.
#[derive(Default)]
pub(crate) struct ContentHashHasher(u64);

impl Hasher for ContentHashHasher {
    fn write(&mut self, bytes: &[u8]) {
        // `[u8; 32]` hashes as its length (`write_usize`) then its bytes.
        let mut word = [0u8; 8];
        let n = bytes.len().min(8);
        word[..n].copy_from_slice(&bytes[..n]);
        self.0 ^= u64::from_le_bytes(word);
    }

    fn write_usize(&mut self, _len: usize) {}

    fn finish(&self) -> u64 {
        self.0
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

    fn put_owned(&self, hash: &[u8; 32], bytes: Vec<u8>) -> DurabilityResult<()> {
        self.blobs.write().map_err(|_| poisoned())?.insert(*hash, bytes);
        Ok(())
    }

    fn get(&self, hash: &[u8; 32]) -> DurabilityResult<Option<Vec<u8>>> {
        Ok(self.blobs.read().map_err(|_| poisoned())?.get(hash).cloned())
    }

    fn has(&self, hash: &[u8; 32]) -> DurabilityResult<bool> {
        Ok(self.blobs.read().map_err(|_| poisoned())?.contains_key(hash))
    }

    fn len_of(&self, hash: &[u8; 32]) -> DurabilityResult<Option<usize>> {
        Ok(self.blobs.read().map_err(|_| poisoned())?.get(hash).map(Vec::len))
    }

    fn delete(&self, hash: &[u8; 32]) -> DurabilityResult<()> {
        self.blobs.write().map_err(|_| poisoned())?.remove(hash);
        Ok(())
    }
}
