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
    positions: HashMap<String, usize>,
}

impl VectorIndex {
    /// A new `dim`-dimensional index ranked by `metric`.
    pub fn new(dim: usize, metric: Metric) -> Self {
        Self {
            dim,
            metric,
            entries: Vec::new(),
            positions: HashMap::new(),
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

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn norm(a: &[f32]) -> f32 {
    dot(a, a).sqrt()
}

fn l2(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| {
            let d = x - y;
            d * d
        })
        .sum::<f32>()
        .sqrt()
}
