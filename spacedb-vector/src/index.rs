//! The on-node vector index — *query embedding in, top-k out, corpus stays*.
//!
//! The index holds the corpus: `(id, embedding)` entries that **never leave the
//! node**. A [`search`](VectorIndex::search) takes a query embedding and returns
//! only the top-k [`Match`]es — ids and similarity scores. The return type makes
//! the privacy property structural: there is no way to read a stored vector back
//! out of a search; the most an authorized caller learns is *which* items are
//! near their query and *how* near.
//!
//! ## Algorithm
//!
//! S3 ships an **exact flat k-NN** search (scan every entry, rank, truncate). For
//! a home-scale corpus that is correct and fast, and — unlike an approximate
//! index — has perfect recall. Sub-linear ANN (HNSW / IVF-flat) is an optimization
//! for very large corpora behind the same [`VectorIndex`] surface, deferred until
//! corpus size demands it.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use crate::error::{VectorError, VectorResult};

/// How similarity is measured. `search` always returns the highest-scoring
/// entries, so every metric is oriented "higher = more similar".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    /// Cosine similarity in `[-1, 1]` (1 = identical direction). Magnitude-invariant.
    Cosine,
    /// Raw dot product (higher = more aligned and larger).
    Dot,
    /// Negative Euclidean distance (0 = identical, more negative = farther).
    Euclidean,
}

impl Metric {
    /// Score `entry` against query `a`, whose norm is `na`. Norms only feed
    /// Cosine: the query's is taken once per search and each entry's once at
    /// insert, rather than both on every comparison — the same arithmetic in
    /// the same order, so the same bits.
    fn score(&self, a: &[f32], na: f32, entry: &Entry) -> f32 {
        let b = &entry.vector;
        match self {
            Metric::Cosine => {
                let nb = entry.norm;
                if na == 0.0 || nb == 0.0 {
                    0.0
                } else {
                    dot(a, b) / (na * nb)
                }
            }
            Metric::Dot => dot(a, b),
            Metric::Euclidean => -l2(a, b),
        }
    }

    /// The norm a vector carries for this metric (only Cosine reads it).
    fn norm_of(&self, v: &[f32]) -> f32 {
        match self {
            Metric::Cosine => norm(v),
            Metric::Dot | Metric::Euclidean => 0.0,
        }
    }
}

/// A search hit: which entry, and how similar. **No vector is returned** — the
/// corpus stays on the node.
#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    pub id: String,
    pub score: f32,
}

struct Entry {
    id: String,
    vector: Vec<f32>,
    /// `Metric::norm_of(vector)` for the index's metric.
    norm: f32,
}

/// An on-node embedding index.
pub struct VectorIndex {
    dim: usize,
    metric: Metric,
    entries: Vec<Entry>,
    /// Where each id sits in `entries`, so an insert finds an existing id
    /// without comparing it against every stored id (a bulk load was O(n^2)).
    /// Hashed with FNV-1a: ids are the index owner's own names, not an
    /// attacker's keys, and SipHash on every insert was 5% of a bulk load.
    positions: HashMap<String, usize, BuildHasherDefault<IdHasher>>,
}

/// FNV-1a over an id's bytes.
#[derive(Default)]
struct IdHasher(u64);

impl Hasher for IdHasher {
    fn write(&mut self, bytes: &[u8]) {
        let mut h = if self.0 == 0 { 0xcbf2_9ce4_8422_2325 } else { self.0 };
        for &b in bytes {
            h = (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
        self.0 = h;
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

impl VectorIndex {
    /// A new `dim`-dimensional index ranked by `metric`.
    pub fn new(dim: usize, metric: Metric) -> Self {
        Self {
            dim,
            metric,
            entries: Vec::new(),
            positions: HashMap::default(),
        }
    }

    pub fn dim(&self) -> usize {
        self.dim
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Insert (or replace) the embedding for `id`. Errors on a dimension mismatch.
    pub fn insert(&mut self, id: impl Into<String>, vector: Vec<f32>) -> VectorResult<()> {
        if vector.len() != self.dim {
            return Err(VectorError::DimMismatch {
                expected: self.dim,
                got: vector.len(),
            });
        }
        let id = id.into();
        let norm = self.metric.norm_of(&vector);
        if let Some(&at) = self.positions.get(&id) {
            let existing = &mut self.entries[at];
            existing.vector = vector;
            existing.norm = norm;
        } else {
            self.positions.insert(id.clone(), self.entries.len());
            self.entries.push(Entry { id, vector, norm });
        }
        Ok(())
    }

    /// Remove `id`. Returns whether it was present.
    pub fn remove(&mut self, id: &str) -> bool {
        let Some(at) = self.positions.remove(id) else {
            return false;
        };
        // Order-preserving, as `retain` was; every later entry moves down one.
        self.entries.remove(at);
        for entry in &self.entries[at..] {
            if let Some(p) = self.positions.get_mut(&entry.id) {
                *p -= 1;
            }
        }
        true
    }

    /// Return the top-`k` entries nearest to `query`, highest score first. Ties
    /// break by id for deterministic (corroboratable) results. Errors on a
    /// dimension mismatch.
    pub fn search(&self, query: &[f32], k: usize) -> VectorResult<Vec<Match>> {
        if query.len() != self.dim {
            return Err(VectorError::DimMismatch {
                expected: self.dim,
                got: query.len(),
            });
        }
        let query_norm = self.metric.norm_of(query);
        // Rank (score, position) pairs; an id is cloned only for the k returned.
        let mut scored: Vec<(f32, usize)> = self
            .entries
            .iter()
            .enumerate()
            .map(|(at, e)| (self.metric.score(query, query_norm, e), at))
            .collect();
        let rank = |a: &(f32, usize), b: &(f32, usize)| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(Ordering::Equal)
                .then_with(|| self.entries[a.1].id.cmp(&self.entries[b.1].id))
        };
        // Ids are unique, so without NaN `rank` is a strict total order and
        // selecting the top k then sorting only those gives exactly the full
        // sort's first k. A NaN score breaks that order; keep the full sort then.
        if k < scored.len() && !scored.iter().any(|(s, _)| s.is_nan()) {
            if k > 0 {
                scored.select_nth_unstable_by(k - 1, rank);
            }
            scored.truncate(k);
        }
        scored.sort_by(rank);
        scored.truncate(k);
        Ok(scored
            .into_iter()
            .map(|(score, at)| Match {
                id: self.entries[at].id.clone(),
                score,
            })
            .collect())
    }
}

/// Lanes of the fixed-width partial sums below. A serial f32 sum is a
/// dependency chain the compiler may not reassociate, so it runs one
/// multiply-add per element; eight independent accumulators, combined in a
/// fixed order, let it vectorize. The split is a constant, not the machine's
/// vector width, so every host computes the same bits.
const LANES: usize = 8;

/// `acc` folded in a fixed tree order, then the scalar tail.
#[inline]
fn fold(acc: [f32; LANES], tail: f32) -> f32 {
    ((acc[0] + acc[4]) + (acc[2] + acc[6])) + ((acc[1] + acc[5]) + (acc[3] + acc[7])) + tail
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = [0f32; LANES];
    let (ca, ra) = a.as_chunks::<LANES>();
    let (cb, rb) = b.as_chunks::<LANES>();
    for (x, y) in ca.iter().zip(cb) {
        for l in 0..LANES {
            acc[l] += x[l] * y[l];
        }
    }
    let tail = ra.iter().zip(rb).fold(0f32, |s, (x, y)| s + x * y);
    fold(acc, tail)
}

/// The one-multiply-add-per-element form `dot` replaced; the test oracle.
#[cfg(test)]
fn dot_serial(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn norm(a: &[f32]) -> f32 {
    dot(a, a).sqrt()
}

fn l2(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = [0f32; LANES];
    let (ca, ra) = a.as_chunks::<LANES>();
    let (cb, rb) = b.as_chunks::<LANES>();
    for (x, y) in ca.iter().zip(cb) {
        for l in 0..LANES {
            let d = x[l] - y[l];
            acc[l] += d * d;
        }
    }
    let tail = ra.iter().zip(rb).fold(0f32, |s, (x, y)| {
        let d = x - y;
        s + d * d
    });
    fold(acc, tail).sqrt()
}

#[cfg(test)]
mod accumulator_tests {
    use super::*;

    fn vectors(n: usize, dim: usize, seed: u64) -> Vec<Vec<f32>> {
        let mut s = seed | 1;
        let mut next = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            ((s >> 40) as f32 / (1u64 << 24) as f32) - 0.5
        };
        (0..n).map(|_| (0..dim).map(|_| next()).collect()).collect()
    }

    #[test]
    fn dot_matches_serial_within_tolerance_at_every_tail() {
        for dim in [1, 7, 8, 9, 15, 16, 64, 65, 100] {
            let vs = vectors(2, dim, dim as u64);
            let (a, b) = (&vs[0], &vs[1]);
            let fast = dot(a, b);
            let slow = dot_serial(a, b);
            let scale = slow.abs().max(1e-3);
            assert!((fast - slow).abs() / scale < 1e-4, "dim {dim}: {fast} vs {slow}");
        }
    }

    #[test]
    fn top_k_sets_agree_with_serial_scoring() {
        // The ranking an index returns under each metric must pick the same
        // ids as serial scoring would, bar exact near-ties (none in this data).
        let dim = 64;
        let corpus = vectors(500, dim, 3);
        let queries = vectors(5, dim, 11);
        for metric in [Metric::Cosine, Metric::Dot, Metric::Euclidean] {
            let mut index = VectorIndex::new(dim, metric);
            for (i, v) in corpus.iter().enumerate() {
                index.insert(format!("v{i}"), v.clone()).unwrap();
            }
            for q in &queries {
                let fast: Vec<String> = index.search(q, 10).unwrap().into_iter().map(|m| m.id).collect();
                let mut slow: Vec<(f32, usize)> = corpus
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        let s = match metric {
                            Metric::Cosine => {
                                dot_serial(q, v) / (dot_serial(q, q).sqrt() * dot_serial(v, v).sqrt())
                            }
                            Metric::Dot => dot_serial(q, v),
                            Metric::Euclidean => -q
                                .iter()
                                .zip(v)
                                .map(|(x, y)| (x - y) * (x - y))
                                .sum::<f32>()
                                .sqrt(),
                        };
                        (s, i)
                    })
                    .collect();
                slow.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
                let slow: Vec<String> = slow.iter().take(10).map(|(_, i)| format!("v{i}")).collect();
                let mut fs = fast.clone();
                let mut ss = slow.clone();
                fs.sort();
                ss.sort();
                assert_eq!(fs, ss, "{metric:?}");
            }
        }
    }
}
