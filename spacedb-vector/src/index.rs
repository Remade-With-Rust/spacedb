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
    fn score(&self, a: &[f32], b: &[f32]) -> f32 {
        match self {
            Metric::Cosine => {
                let (na, nb) = (norm(a), norm(b));
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
}

/// An on-node embedding index.
pub struct VectorIndex {
    dim: usize,
    metric: Metric,
    entries: Vec<Entry>,
}

impl VectorIndex {
    /// A new `dim`-dimensional index ranked by `metric`.
    pub fn new(dim: usize, metric: Metric) -> Self {
        Self {
            dim,
            metric,
            entries: Vec::new(),
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
        if let Some(existing) = self.entries.iter_mut().find(|e| e.id == id) {
            existing.vector = vector;
        } else {
            self.entries.push(Entry { id, vector });
        }
        Ok(())
    }

    /// Remove `id`. Returns whether it was present.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        self.entries.len() != before
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
        let mut scored: Vec<Match> = self
            .entries
            .iter()
            .map(|e| Match {
                id: e.id.clone(),
                score: self.metric.score(query, &e.vector),
            })
            .collect();
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        });
        scored.truncate(k);
        Ok(scored)
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
