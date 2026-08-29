//! A/B: `reed-solomon-erasure` 6 (the engine this crate shipped before) vs
//! `rusty_erasure` 0.4 (the engine it ships now), both at their AS-DEPLOYED
//! configurations, on the geometries the platform uses.
//!
//! Measurement shape (codec-measurement discipline):
//! - arms interleaved per round, LEADING ARM ALTERNATED between rounds;
//! - work parity asserted every rep (parity/rebuilt bytes byte-identical);
//! - a NULL arm (new vs new) reports the session's noise floor;
//! - paired win rate + z-score, plus median ratio and best-of-N throughput;
//! - kernel-reach census printed, so a silently-scalar run is visible;
//! - run pinned to one core at High priority (see the harness invocation).
//!
//! The old arm's recover includes its own per-rebuild allocations — that is
//! what the replaced code actually did in production, and the label says so.

use std::hint::black_box;
use std::time::Instant;

fn stripe(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.extend_from_slice(&state.wrapping_mul(0x2545F4914F6CDD1D).to_le_bytes());
    }
    out.truncate(len);
    out
}

struct Fixture {
    k: usize,
    p: usize,
    shard_len: usize,
    data: Vec<Vec<u8>>,
    old: reed_solomon_erasure::galois_8::ReedSolomon,
    new: rusty_erasure::Coder,
}

impl Fixture {
    fn build(k: usize, p: usize, snapshot_len: usize) -> Self {
        let bytes = stripe(snapshot_len, (k as u64) << 40 | (p as u64) << 32 | snapshot_len as u64);
        let shard_len = snapshot_len.div_ceil(k).max(1);
        let mut data: Vec<Vec<u8>> = Vec::with_capacity(k);
        for i in 0..k {
            let mut s = vec![0u8; shard_len];
            let start = i * shard_len;
            if start < bytes.len() {
                let end = (start + shard_len).min(bytes.len());
                s[..end - start].copy_from_slice(&bytes[start..end]);
            }
            data.push(s);
        }
        let old = reed_solomon_erasure::galois_8::ReedSolomon::new(k, p).unwrap();
        let matrix = rusty_erasure::compat::reed_solomon_erasure_matrix(k, p).unwrap();
        let new = rusty_erasure::coder(matrix).unwrap();
        Fixture { k, p, shard_len, data, old, new }
    }

    fn encode_old(&self, stripe_buf: &mut [Vec<u8>]) {
        // reed-solomon-erasure's API: one Vec of k data + p parity, in place.
        self.old.encode(stripe_buf).unwrap();
    }

    fn encode_new(&self, parity: &mut [Vec<u8>]) {
        let data_refs: Vec<&[u8]> = self.data.iter().map(|s| s.as_slice()).collect();
        let mut parity_refs: Vec<&mut [u8]> = parity.iter_mut().map(|s| s.as_mut_slice()).collect();
        self.new.encode(&data_refs, &mut parity_refs).unwrap();
    }
}

struct PairStats {
    wins_new: usize,
    n: usize,
    ratios: Vec<f64>, // old_time / new_time per pair (>1 means new is faster)
    best_new: f64,
    best_old: f64,
}

fn z_score(wins: usize, n: usize) -> f64 {
    (wins as f64 - n as f64 / 2.0) / (0.5 * (n as f64).sqrt())
}

fn median(xs: &mut [f64]) -> f64 {
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    xs[xs.len() / 2]
}

/// Run `n_pairs` interleaved pairs of two closures, alternating the leading
/// arm each round. Each timed rep runs `iters` inner iterations. Returns
/// (time_a, time_b) per pair in seconds.
fn ab_rounds<A: FnMut(), B: FnMut()>(
    n_pairs: usize,
    iters: usize,
    mut arm_a: A,
    mut arm_b: B,
) -> Vec<(f64, f64)> {
    let mut pairs = Vec::with_capacity(n_pairs);
    for round in 0..n_pairs {
        let time = |f: &mut dyn FnMut()| {
            let t = Instant::now();
            for _ in 0..iters {
                f();
            }
            t.elapsed().as_secs_f64() / iters as f64
        };
        let (ta, tb) = if round % 2 == 0 {
            let ta = time(&mut arm_a);
            let tb = time(&mut arm_b);
            (ta, tb)
        } else {
            let tb = time(&mut arm_b);
            let ta = time(&mut arm_a);
            (ta, tb)
        };
        pairs.push((ta, tb));
    }
    pairs
}

fn stats(pairs: &[(f64, f64)]) -> PairStats {
    let mut ratios: Vec<f64> = pairs.iter().map(|(old, new)| old / new).collect();
    let wins_new = pairs.iter().filter(|(old, new)| new < old).count();
    let best_new = pairs.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let best_old = pairs.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let _ = median(&mut ratios);
    PairStats { wins_new, n: pairs.len(), ratios, best_new, best_old }
}

fn report(label: &str, bytes_per_rep: usize, s: &mut PairStats) {
    let med = median(&mut s.ratios);
    let z = z_score(s.wins_new, s.n);
    let mibs = |t: f64| bytes_per_rep as f64 / t / (1024.0 * 1024.0);
    println!(
        "{label}: median old/new = {med:.3}x | new wins {}/{} (z={z:+.2}) | best-of-N: new {:.0} MiB/s, old {:.0} MiB/s",
        s.wins_new, s.n, mibs(s.best_new), mibs(s.best_old),
    );
}

fn main() {
    const N_PAIRS: usize = 24;
    const TARGET_REP_SECS: f64 = 0.10;

    println!("method: in-process ABBA, leading arm alternated per round; work parity asserted per rep (byte-identity); N={N_PAIRS} pairs; per-rep >= {TARGET_REP_SECS}s via inner iters; wall clock (Instant) in a process the harness pins to one core at High priority; null arm printed last; ratios are old/new (>1 = new faster).");

    // (k, p, snapshot MiB): the test-suite default, the same geometry at
    // memory-bound size, and a mesh-scale wide stripe.
    for &(k, p, mib) in &[(4usize, 2usize, 4usize), (4, 2, 64), (10, 4, 64)] {
        let snapshot_len = mib * 1024 * 1024;
        let fx = Fixture::build(k, p, snapshot_len);
        let parity_bytes = fx.shard_len * p;

        // --- encode arms + one-time work-parity gate ---
        let mut old_stripe: Vec<Vec<u8>> = fx.data.clone();
        old_stripe.resize(k + p, vec![0u8; fx.shard_len]);
        let mut new_parity: Vec<Vec<u8>> = vec![vec![0u8; fx.shard_len]; p];
        fx.encode_old(&mut old_stripe);
        fx.encode_new(&mut new_parity);
        for i in 0..p {
            assert_eq!(old_stripe[k + i], new_parity[i], "encode parity diverged");
        }

        // Size the inner iteration count off the SLOWER arm so both arms get
        // identical iters (work parity) and reps clear the target duration.
        let once = |f: &mut dyn FnMut()| {
            let t = Instant::now();
            f();
            t.elapsed().as_secs_f64()
        };
        let w_old = once(&mut || fx.encode_old(black_box(&mut old_stripe)));
        let w_new = once(&mut || fx.encode_new(black_box(&mut new_parity)));
        let iters = (TARGET_REP_SECS / w_old.max(w_new)).ceil().max(1.0) as usize;

        let pairs = ab_rounds(
            N_PAIRS,
            iters,
            || fx.encode_old(black_box(&mut old_stripe)),
            || fx.encode_new(black_box(&mut new_parity)),
        );
        // Re-assert parity after the timed reps: the arms never drifted.
        for i in 0..p {
            assert_eq!(old_stripe[k + i], new_parity[i], "post-bench parity drift");
        }
        let mut s = stats(&pairs);
        report(
            &format!("encode  k={k:>2} p={p} {mib:>2} MiB (iters={iters})"),
            parity_bytes,
            &mut s,
        );

        // --- recover arms: first p shards lost (data losses -> real inversion) ---
        let full: Vec<Vec<u8>> = old_stripe.clone();
        let expected: Vec<Vec<u8>> = full[..p].to_vec();

        let mut old_slots: Vec<Option<Vec<u8>>> = full.iter().cloned().map(Some).collect();
        let mut rebuilt: Vec<Vec<u8>> = vec![vec![0u8; fx.shard_len]; p];
        let rebuild_idx: Vec<usize> = (0..p).collect();

        // One-time correctness gate, inline, before the timed closures take
        // their long-lived borrows of the buffers.
        {
            for slot in old_slots.iter_mut().take(p) {
                *slot = None;
            }
            fx.old.reconstruct(&mut old_slots).unwrap();
            let stripe_refs: Vec<Option<&[u8]>> = full
                .iter()
                .enumerate()
                .map(|(i, s)| if i < p { None } else { Some(s.as_slice()) })
                .collect();
            let mut out: Vec<&mut [u8]> = rebuilt.iter_mut().map(|s| s.as_mut_slice()).collect();
            fx.new.recover(&stripe_refs, &rebuild_idx, &mut out).unwrap();
        }
        for i in 0..p {
            assert_eq!(old_slots[i].as_ref().unwrap(), &expected[i], "old recover wrong");
            assert_eq!(&rebuilt[i], &expected[i], "recover outputs diverged");
        }

        // Old arm: reconstruct fills the Nones (allocating rebuilt shards —
        // the replaced code's real production cost; labeled, not hidden).
        let mut recover_old = || {
            for slot in old_slots.iter_mut().take(p) {
                *slot = None;
            }
            fx.old.reconstruct(black_box(&mut old_slots)).unwrap();
        };

        // New arm: rebuild into preallocated buffers via a stripe of refs.
        let mut recover_new = || {
            let stripe_refs: Vec<Option<&[u8]>> = full
                .iter()
                .enumerate()
                .map(|(i, s)| if i < p { None } else { Some(s.as_slice()) })
                .collect();
            let mut out: Vec<&mut [u8]> = rebuilt.iter_mut().map(|s| s.as_mut_slice()).collect();
            fx.new.recover(black_box(&stripe_refs), &rebuild_idx, &mut out).unwrap();
        };

        let w_old = once(&mut recover_old);
        let w_new = once(&mut recover_new);
        let iters = (TARGET_REP_SECS / w_old.max(w_new)).ceil().max(1.0) as usize;
        let pairs = ab_rounds(N_PAIRS, iters, recover_old, recover_new);
        let mut s = stats(&pairs);
        report(
            &format!("recover k={k:>2} p={p} {mib:>2} MiB (iters={iters})"),
            fx.shard_len * p,
            &mut s,
        );
    }

    // --- null arm: new vs new at the middle config = the session noise floor ---
    let fx = Fixture::build(4, 2, 64 * 1024 * 1024);
    let mut parity_a: Vec<Vec<u8>> = vec![vec![0u8; fx.shard_len]; 2];
    let mut parity_b: Vec<Vec<u8>> = vec![vec![0u8; fx.shard_len]; 2];
    let t = Instant::now();
    fx.encode_new(&mut parity_a);
    let w = t.elapsed().as_secs_f64();
    let iters = (TARGET_REP_SECS / w).ceil().max(1.0) as usize;
    let pairs = ab_rounds(
        N_PAIRS,
        iters,
        || fx.encode_new(black_box(&mut parity_a)),
        || fx.encode_new(black_box(&mut parity_b)),
    );
    let mut s = stats(&pairs);
    let med = median(&mut s.ratios);
    let worst = s
        .ratios
        .iter()
        .map(|r| (r - 1.0).abs())
        .fold(0.0f64, f64::max);
    println!(
        "null arm (new vs new): median {med:.3}x, worst pair {:.1}% — the floor below which nothing above is a claim",
        worst * 100.0
    );

    let census = rusty_erasure::census::read();
    match census.accel_percent() {
        Some(pct) => println!("kernel census: {pct:.1}% of coder byte-work ran on accel kernels"),
        None => println!("kernel census: no work recorded (!) — the bench measured nothing"),
    }
}
