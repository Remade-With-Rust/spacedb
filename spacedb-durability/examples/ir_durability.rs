//! Deterministic instruction-count driver for the in-crate erasure path —
//! `encode_snapshot` / `reconstruct_snapshot` (callgrind Ir). `engine_ab` drives
//! the coders directly and never reaches this code.

use spacedb_durability::{encode_snapshot, reconstruct_snapshot};

fn snapshot(len: usize, seed: u64) -> Vec<u8> {
    let mut s = seed | 1;
    (0..len)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s as u8
        })
        .collect()
}

fn main() {
    let mut check = 0u64;
    let mut rebuilt = 0u64;
    // Small snapshots (setup-dominated) and a larger one (kernel-dominated).
    for (len, reps, k, p) in [
        (4 * 1024, 40, 5, 3),
        (64 * 1024, 10, 8, 4),
        (1024, 60, 3, 2),
    ] {
        for r in 0..reps {
            let snap = snapshot(len, (len + r) as u64);
            let (manifest, shards) = encode_snapshot(&snap, k, p).unwrap();
            // Lose the first `p` shards (data shards: forces a real rebuild).
            let available: Vec<_> = shards.into_iter().skip(p).collect();
            let back = reconstruct_snapshot(&manifest, &available).unwrap();
            assert_eq!(back, snap);
            check = check.wrapping_add(back.iter().map(|&b| b as u64).sum::<u64>());
            rebuilt += 1;
        }
    }
    println!("rebuilt {rebuilt} check {check}");
}
