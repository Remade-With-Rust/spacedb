//! Migration oracle: `rusty_erasure` (klauspost construction) must be
//! byte-compatible with the `reed-solomon-erasure` crate this library used
//! before — in BOTH directions — or shards placed on the mesh before the
//! migration would stop reconstructing after it (and vice versa during a
//! mixed-version fleet upgrade).
//!
//! Three gates, over a (k, p) × length grid that covers the padding edges:
//!
//! 1. **Parity byte-identity** — encoding the same stripe through both
//!    engines yields identical bytes for every shard.
//! 2. **Old shards, new decoder** — a stripe encoded by
//!    `reed-solomon-erasure` reconstructs through the shipping
//!    [`reconstruct_snapshot`] path after `p` losses.
//! 3. **New shards, old decoder** — a stripe encoded by the shipping
//!    [`encode_snapshot`] reconstructs through `reed-solomon-erasure` after
//!    `p` losses.
//!
//! This dev-dependency oracle is scheduled for removal one published release
//! after the migration ships (see docs/plans/rusty_time_deploy.md, W1.1).

use spacedb_durability::{encode_snapshot, reconstruct_snapshot};

/// Deterministic pseudo-random bytes (xorshift64*) — seeded, so every run and
/// both engines face the identical stripe.
fn stripe(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.extend_from_slice(&state.wrapping_mul(0x2545F4914F6CDD1D).to_le_bytes());
    }
    out.truncate(len);
    out
}

/// The geometries the platform uses (durability tests, maestro-db defaults)
/// plus the field-stressing corners, and lengths that hit the zero-padding
/// edge (`len % k != 0`), the sub-`k` edge, and a multi-KiB body.
const GRID: &[(usize, usize)] = &[(2, 1), (3, 2), (4, 2), (4, 4), (10, 4), (16, 8)];
const LENS: &[usize] = &[1, 3, 1024, 4096 + 7, 65536 + 13];

/// Encode a stripe with the OLD crate, returning all `k + p` shards.
fn old_encode(data: &[u8], k: usize, p: usize) -> Vec<Vec<u8>> {
    let shard_len = data.len().div_ceil(k).max(1);
    let mut shards: Vec<Vec<u8>> = Vec::with_capacity(k + p);
    for i in 0..k {
        let mut shard = vec![0u8; shard_len];
        let start = i * shard_len;
        if start < data.len() {
            let end = (start + shard_len).min(data.len());
            shard[..end - start].copy_from_slice(&data[start..end]);
        }
        shards.push(shard);
    }
    shards.resize(k + p, vec![0u8; shard_len]);
    let rs = reed_solomon_erasure::galois_8::ReedSolomon::new(k, p).unwrap();
    rs.encode(&mut shards).unwrap();
    shards
}

#[test]
fn parity_is_byte_identical_across_engines() {
    let mut cells = 0usize;
    for &(k, p) in GRID {
        for &len in LENS {
            let data = stripe(len, (k as u64) << 32 | (p as u64) << 16 | len as u64);
            let old = old_encode(&data, k, p);
            let (_, new) = encode_snapshot(&data, k, p).unwrap();
            assert_eq!(new.len(), k + p);
            for (i, shard) in new.iter().enumerate() {
                assert_eq!(
                    shard.bytes, old[i],
                    "shard {i} diverges at k={k} p={p} len={len}"
                );
            }
            cells += 1;
        }
    }
    // The grid actually ran — a silently-skipped gate is worse than no gate.
    assert_eq!(cells, GRID.len() * LENS.len());
}

#[test]
fn old_shards_reconstruct_through_the_new_decoder() {
    for &(k, p) in GRID {
        let len = 4096 + 7;
        let data = stripe(len, 0xA11CE ^ ((k as u64) << 8 | p as u64));
        let old = old_encode(&data, k, p);

        // Manifest for the old stripe via the shipping encoder — legitimate
        // because parity_is_byte_identical_across_engines proves the shard
        // bytes (and so the hashes) agree.
        let (manifest, _) = encode_snapshot(&data, k, p).unwrap();

        // Worst case: drop the FIRST p shards (all-data losses where p <= k),
        // keeping the last k — forces real matrix inversion, not a copy-out.
        let survivors: Vec<spacedb_durability::Shard> = old
            .iter()
            .enumerate()
            .skip(p)
            .map(|(i, bytes)| spacedb_durability::Shard {
                index: i as u16,
                bytes: bytes.clone(),
            })
            .collect();
        let rebuilt = reconstruct_snapshot(&manifest, &survivors).unwrap();
        assert_eq!(rebuilt, data, "old-encoded stripe lost k={k} p={p}");
    }
}

#[test]
fn new_shards_reconstruct_through_the_old_decoder() {
    for &(k, p) in GRID {
        let len = 4096 + 7;
        let data = stripe(len, 0xB0B ^ ((k as u64) << 8 | p as u64));
        let (manifest, new) = encode_snapshot(&data, k, p).unwrap();
        let shard_len = manifest.shard_len as usize;

        // Same worst case: first p shards lost.
        let mut slots: Vec<Option<Vec<u8>>> = new
            .into_iter()
            .map(|s| Some(s.bytes))
            .collect();
        for slot in slots.iter_mut().take(p) {
            *slot = None;
        }
        let rs = reed_solomon_erasure::galois_8::ReedSolomon::new(k, p).unwrap();
        rs.reconstruct(&mut slots).unwrap();

        let mut rebuilt = Vec::with_capacity(k * shard_len);
        for slot in slots.iter().take(k) {
            rebuilt.extend_from_slice(slot.as_ref().unwrap());
        }
        rebuilt.truncate(manifest.snapshot_len as usize);
        assert_eq!(rebuilt, data, "new-encoded stripe lost k={k} p={p}");
    }
}
