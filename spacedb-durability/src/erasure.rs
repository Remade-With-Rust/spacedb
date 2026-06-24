//! Content-addressed, Reed-Solomon erasure-coded snapshots — the foundation of
//! mesh durability.
//!
//! A **snapshot** is opaque bytes (in the real system, the AEAD-sealed state of a
//! dataset — already ciphertext, so erasure operates on ciphertext and every
//! shard is unreadable to whoever holds it). [`encode_snapshot`] splits it into
//! `k` data shards and adds `parity` parity shards (`n = k + parity` total); any
//! **`k` of the `n`** shards reconstruct the original, so up to `parity` shards
//! can be lost. [`reconstruct_snapshot`] rebuilds from the survivors and verifies
//! every byte against the [`Manifest`].
//!
//! The manifest is the small, durable description of how to rebuild: the
//! snapshot's BLAKE3 hash and length, the `(k, parity)` parameters, the shard
//! length, and each shard's BLAKE3 hash. Shards are content-addressed, so a
//! corrupt or tampered shard is detected (by hash) *before* it can poison
//! reconstruction, and the rebuilt snapshot is itself hash-verified.

use serde::{Deserialize, Serialize};

use crate::error::{DurabilityError, DurabilityResult};

/// Maximum total shards for the GF(2^8) Reed-Solomon field.
const MAX_TOTAL_SHARDS: usize = 256;

pub(crate) fn content_hash(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}

/// One erasure-coded shard: its position in the code and its bytes (a ciphertext
/// fragment). Stored on a host, which cannot read it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shard {
    /// Position in the code, `0..n` (data shards first, then parity).
    pub index: u16,
    /// The shard bytes (all shards in a snapshot share the same length).
    pub bytes: Vec<u8>,
}

/// A manifest's reference to one shard: its index and content hash. This is how a
/// provided shard is verified before use.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShardRef {
    pub index: u16,
    pub hash: [u8; 32],
}

/// The durable description of an erasure-coded snapshot — everything needed to
/// verify and reconstruct it from a sufficient subset of shards.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// BLAKE3 hash of the original snapshot bytes.
    pub snapshot_hash: [u8; 32],
    /// Length of the original snapshot in bytes (shards are zero-padded to a
    /// multiple of the shard length; this is the truncation point on rebuild).
    pub snapshot_len: u64,
    /// `k` — the number of data shards; also the number required to reconstruct.
    pub data_shards: u16,
    /// `n - k` — the number of parity shards; also the number of losses tolerated.
    pub parity_shards: u16,
    /// The length of every shard in bytes.
    pub shard_len: u64,
    /// One reference per shard, ordered by index (`0..n`).
    pub shards: Vec<ShardRef>,
}

impl Manifest {
    /// Total shards `n = k + parity`.
    pub fn total_shards(&self) -> usize {
        self.data_shards as usize + self.parity_shards as usize
    }

    /// Shards required to reconstruct (`k`).
    pub fn shards_needed(&self) -> usize {
        self.data_shards as usize
    }

    /// How many shard losses can be tolerated (`n - k = parity`).
    pub fn fault_tolerance(&self) -> usize {
        self.parity_shards as usize
    }

    /// Serialize the manifest (postcard).
    pub fn encode(&self) -> DurabilityResult<Vec<u8>> {
        postcard::to_allocvec(self).map_err(|e| DurabilityError::Manifest(e.to_string()))
    }

    /// Deserialize a manifest (postcard).
    pub fn decode(bytes: &[u8]) -> DurabilityResult<Self> {
        postcard::from_bytes(bytes).map_err(|e| DurabilityError::Manifest(e.to_string()))
    }
}

/// Erasure-code `snapshot` into `data_shards` data + `parity_shards` parity
/// shards, returning the [`Manifest`] and the `n` shards. Any `data_shards` of
/// them reconstruct the original.
pub fn encode_snapshot(
    snapshot: &[u8],
    data_shards: usize,
    parity_shards: usize,
) -> DurabilityResult<(Manifest, Vec<Shard>)> {
    if data_shards < 1 {
        return Err(DurabilityError::InvalidParams("need at least 1 data shard".into()));
    }
    if parity_shards < 1 {
        return Err(DurabilityError::InvalidParams(
            "need at least 1 parity shard (no redundancy otherwise)".into(),
        ));
    }
    let total = data_shards + parity_shards;
    if total > MAX_TOTAL_SHARDS {
        return Err(DurabilityError::InvalidParams(format!(
            "data + parity = {total} exceeds the {MAX_TOTAL_SHARDS}-shard limit"
        )));
    }

    let rs = reed_solomon_erasure::galois_8::ReedSolomon::new(data_shards, parity_shards)
        .map_err(|e| DurabilityError::Erasure(e.to_string()))?;

    let snapshot_len = snapshot.len();
    // Every shard is the same length; the data is split across `k` shards and
    // zero-padded. At least 1 byte so the codec has something to work on.
    let shard_len = snapshot_len.div_ceil(data_shards).max(1);

    let mut shards: Vec<Vec<u8>> = Vec::with_capacity(total);
    for i in 0..data_shards {
        let mut shard = vec![0u8; shard_len];
        let start = i * shard_len;
        if start < snapshot_len {
            let end = (start + shard_len).min(snapshot_len);
            shard[..end - start].copy_from_slice(&snapshot[start..end]);
        }
        shards.push(shard);
    }
    for _ in 0..parity_shards {
        shards.push(vec![0u8; shard_len]);
    }

    rs.encode(&mut shards)
        .map_err(|e| DurabilityError::Erasure(e.to_string()))?;

    let shard_refs: Vec<ShardRef> = shards
        .iter()
        .enumerate()
        .map(|(i, bytes)| ShardRef {
            index: i as u16,
            hash: content_hash(bytes),
        })
        .collect();
    let out_shards: Vec<Shard> = shards
        .into_iter()
        .enumerate()
        .map(|(i, bytes)| Shard {
            index: i as u16,
            bytes,
        })
        .collect();

    let manifest = Manifest {
        snapshot_hash: content_hash(snapshot),
        snapshot_len: snapshot_len as u64,
        data_shards: data_shards as u16,
        parity_shards: parity_shards as u16,
        shard_len: shard_len as u64,
        shards: shard_refs,
    };

    Ok((manifest, out_shards))
}

/// Reconstruct the original snapshot from `available` shards, using `manifest`.
/// Every provided shard is verified against the manifest (a tampered shard is
/// rejected before use), and the rebuilt snapshot is hash-verified. Errors with
/// [`DurabilityError::InsufficientShards`] if fewer than `k` distinct valid
/// shards are present.
pub fn reconstruct_snapshot(
    manifest: &Manifest,
    available: &[Shard],
) -> DurabilityResult<Vec<u8>> {
    let k = manifest.data_shards as usize;
    let total = manifest.total_shards();
    let shard_len = manifest.shard_len as usize;

    let mut slots: Vec<Option<Vec<u8>>> = vec![None; total];
    let mut have = 0usize;

    for shard in available {
        let index = shard.index as usize;
        if index >= total {
            return Err(DurabilityError::InvalidParams(format!(
                "shard index {index} is out of range for {total} shards"
            )));
        }
        if shard.bytes.len() != shard_len {
            return Err(DurabilityError::InvalidParams(format!(
                "shard {index} has length {} (expected {shard_len})",
                shard.bytes.len()
            )));
        }
        let expected = manifest
            .shards
            .iter()
            .find(|r| r.index as usize == index)
            .ok_or_else(|| DurabilityError::Manifest(format!("no ref for shard {index}")))?;
        if content_hash(&shard.bytes) != expected.hash {
            return Err(DurabilityError::ShardHashMismatch { index: shard.index });
        }
        if slots[index].is_none() {
            have += 1;
        }
        slots[index] = Some(shard.bytes.clone());
    }

    if have < k {
        return Err(DurabilityError::InsufficientShards { have, need: k });
    }

    let rs = reed_solomon_erasure::galois_8::ReedSolomon::new(k, manifest.parity_shards as usize)
        .map_err(|e| DurabilityError::Erasure(e.to_string()))?;
    rs.reconstruct(&mut slots)
        .map_err(|e| DurabilityError::Erasure(e.to_string()))?;

    let mut snapshot = Vec::with_capacity(k * shard_len);
    for (i, slot) in slots.iter().take(k).enumerate() {
        let bytes = slot.as_ref().ok_or_else(|| {
            DurabilityError::Erasure(format!("data shard {i} missing after reconstruct"))
        })?;
        snapshot.extend_from_slice(bytes);
    }
    snapshot.truncate(manifest.snapshot_len as usize);

    if content_hash(&snapshot) != manifest.snapshot_hash {
        return Err(DurabilityError::SnapshotHashMismatch);
    }

    Ok(snapshot)
}
