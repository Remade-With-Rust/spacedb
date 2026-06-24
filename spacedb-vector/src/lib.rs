#![forbid(unsafe_code)]
//! # spacedb-vector — SpaceDB Layer 4 (the private-RAG substrate)
//!
//! A native, on-node vector index co-located with the data: a query embedding
//! goes in, and **only the top-k results come out** — the corpus never leaves the
//! home. An authorized AI agent's retrieval runs here, **gated by an mID
//! capability** ([`retrieve`]), which is the concrete mechanism behind
//! "inaccessible by default, accessible by mID-gated consent": semantic access to
//! *results*, not bulk data.
//!
//! Open-core (MIT): depends only on `spacedb-access` (for the capability gate).
//! S3 ships an exact flat k-NN [`VectorIndex`]; sub-linear ANN (HNSW / IVF) is a
//! future optimization behind the same surface.

mod error;
pub use error::{VectorError, VectorResult};

mod index;
pub use index::{Match, Metric, VectorIndex};

mod retrieve;
pub use retrieve::retrieve;
