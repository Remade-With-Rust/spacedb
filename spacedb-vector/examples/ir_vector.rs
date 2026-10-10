//! Deterministic instruction-count driver for brute-force vector search
//! (callgrind Ir). Score bits are folded into the anchor, so a change that
//! alters any ranking or any score moves it.

use spacedb_vector::{Metric, VectorIndex};

fn main() {
    let dim = 64;
    let n = 1500;
    let mut s = 0x9E3779B97F4A7C15u64;
    let mut next = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        ((s >> 40) as f32 / (1u64 << 24) as f32) - 0.5
    };
    let vectors: Vec<Vec<f32>> = (0..n).map(|_| (0..dim).map(|_| next()).collect()).collect();
    let queries: Vec<Vec<f32>> = (0..12)
        .map(|_| (0..dim).map(|_| next()).collect())
        .collect();

    let mut check = 0u64;
    let mut found = 0usize;
    for metric in [Metric::Cosine, Metric::Dot, Metric::Euclidean] {
        let mut index = VectorIndex::new(dim, metric);
        for (i, v) in vectors.iter().enumerate() {
            index.insert(format!("item-{i:05}"), v.clone()).unwrap();
        }
        for q in &queries {
            for m in index.search(q, 10).unwrap() {
                check = (check ^ m.score.to_bits() as u64).wrapping_mul(0x100000001b3);
                check ^= m.id.len() as u64 + m.id.as_bytes()[9] as u64;
                found += 1;
            }
        }
    }
    println!("found {found} check {check:016x}");
}
