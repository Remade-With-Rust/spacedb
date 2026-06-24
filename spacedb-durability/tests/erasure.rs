//! M4-S1: the durability foundation. The headline invariant — *any k-of-n
//! survivors reconstruct the exact original* — is fuzzed; the edges (insufficient
//! shards, tampering, padding, empty, bad params) are pinned down explicitly.

use proptest::prelude::*;
use spacedb_durability::{
    encode_snapshot, reconstruct_snapshot, DurabilityError, Manifest, Shard,
};

fn sample(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 + 3) as u8).collect()
}

#[test]
fn round_trips_with_all_shards() {
    let data = sample(1000);
    let (manifest, shards) = encode_snapshot(&data, 4, 2).unwrap();
    assert_eq!(manifest.total_shards(), 6);
    assert_eq!(manifest.shards_needed(), 4);
    assert_eq!(manifest.fault_tolerance(), 2);
    assert_eq!(reconstruct_snapshot(&manifest, &shards).unwrap(), data);
}

#[test]
fn reconstructs_from_exactly_k_shards() {
    let data = sample(1000);
    let (manifest, shards) = encode_snapshot(&data, 4, 2).unwrap();
    // keep only the last 4 (drop the 2 we're allowed to lose)
    let kept: Vec<Shard> = shards.into_iter().skip(2).collect();
    assert_eq!(kept.len(), 4);
    assert_eq!(reconstruct_snapshot(&manifest, &kept).unwrap(), data);
}

#[test]
fn reconstructs_when_all_data_shards_are_lost() {
    // Drop every data shard (0..4); rebuild purely from the 4 parity shards of a
    // k=4, parity=4 code — the hardest reconstruction.
    let data = sample(777);
    let (manifest, shards) = encode_snapshot(&data, 4, 4).unwrap();
    let parity_only: Vec<Shard> = shards.into_iter().filter(|s| s.index >= 4).collect();
    assert_eq!(parity_only.len(), 4);
    assert_eq!(reconstruct_snapshot(&manifest, &parity_only).unwrap(), data);
}

#[test]
fn fewer_than_k_shards_is_insufficient() {
    let data = sample(500);
    let (manifest, shards) = encode_snapshot(&data, 4, 2).unwrap();
    // keep only 3 (need 4)
    let kept: Vec<Shard> = shards.into_iter().take(3).collect();
    let err = reconstruct_snapshot(&manifest, &kept).unwrap_err();
    assert!(matches!(
        err,
        DurabilityError::InsufficientShards { have: 3, need: 4 }
    ));
}

#[test]
fn a_tampered_shard_is_rejected() {
    let data = sample(500);
    let (manifest, mut shards) = encode_snapshot(&data, 4, 2).unwrap();
    shards[1].bytes[0] ^= 0x01; // flip a byte
    let err = reconstruct_snapshot(&manifest, &shards).unwrap_err();
    assert!(matches!(err, DurabilityError::ShardHashMismatch { index: 1 }));
}

#[test]
fn a_corrupt_manifest_snapshot_hash_is_caught() {
    let data = sample(500);
    let (mut manifest, shards) = encode_snapshot(&data, 4, 2).unwrap();
    manifest.snapshot_hash[0] ^= 0x01;
    let err = reconstruct_snapshot(&manifest, &shards).unwrap_err();
    assert!(matches!(err, DurabilityError::SnapshotHashMismatch));
}

#[test]
fn duplicate_shards_do_not_inflate_the_count() {
    let data = sample(400);
    let (manifest, shards) = encode_snapshot(&data, 4, 2).unwrap();
    // three distinct shards, one of them provided twice -> still only 3 distinct
    let kept = vec![
        shards[0].clone(),
        shards[1].clone(),
        shards[2].clone(),
        shards[2].clone(),
    ];
    let err = reconstruct_snapshot(&manifest, &kept).unwrap_err();
    assert!(matches!(
        err,
        DurabilityError::InsufficientShards { have: 3, need: 4 }
    ));
}

#[test]
fn empty_snapshot_round_trips() {
    let data: Vec<u8> = Vec::new();
    let (manifest, shards) = encode_snapshot(&data, 3, 2).unwrap();
    assert_eq!(manifest.snapshot_len, 0);
    let kept: Vec<Shard> = shards.into_iter().skip(2).collect(); // drop 2, keep 3 = k
    assert_eq!(reconstruct_snapshot(&manifest, &kept).unwrap(), data);
}

#[test]
fn length_not_divisible_by_k_round_trips_exactly() {
    // 1001 bytes across 4 data shards -> padding; truncation must be exact.
    let data = sample(1001);
    let (manifest, shards) = encode_snapshot(&data, 4, 3).unwrap();
    assert!(manifest.shard_len * (manifest.data_shards as u64) >= manifest.snapshot_len);
    let kept: Vec<Shard> = shards.into_iter().skip(3).collect();
    let out = reconstruct_snapshot(&manifest, &kept).unwrap();
    assert_eq!(out.len(), 1001);
    assert_eq!(out, data);
}

#[test]
fn invalid_params_are_rejected() {
    assert!(matches!(
        encode_snapshot(b"x", 0, 2),
        Err(DurabilityError::InvalidParams(_))
    ));
    assert!(matches!(
        encode_snapshot(b"x", 4, 0),
        Err(DurabilityError::InvalidParams(_))
    ));
    assert!(matches!(
        encode_snapshot(b"x", 200, 200),
        Err(DurabilityError::InvalidParams(_))
    ));
}

#[test]
fn manifest_codec_round_trips() {
    let data = sample(300);
    let (manifest, _shards) = encode_snapshot(&data, 3, 2).unwrap();
    let bytes = manifest.encode().unwrap();
    assert_eq!(Manifest::decode(&bytes).unwrap(), manifest);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(250))]

    /// The core durability invariant: for any snapshot and any valid (k, parity),
    /// dropping any `parity` shards still reconstructs the exact original.
    #[test]
    fn any_k_of_n_survivors_reconstruct_the_original(
        data in prop::collection::vec(any::<u8>(), 0..3000),
        k in 1usize..8,
        parity in 1usize..8,
        drop_rotation in 0usize..16,
    ) {
        let (manifest, shards) = encode_snapshot(&data, k, parity).unwrap();
        let n = k + parity;

        // Drop exactly `parity` shards (the maximum tolerable), chosen by a
        // rotating offset so different runs lose different positions — including
        // runs that lose data shards, parity shards, or a mix.
        let drop_start = drop_rotation % n;
        let dropped: std::collections::HashSet<usize> =
            (0..parity).map(|i| (drop_start + i) % n).collect();
        let survivors: Vec<Shard> = shards
            .into_iter()
            .filter(|s| !dropped.contains(&(s.index as usize)))
            .collect();
        prop_assert_eq!(survivors.len(), k);

        let recovered = reconstruct_snapshot(&manifest, &survivors).unwrap();
        prop_assert_eq!(recovered, data);
    }
}
