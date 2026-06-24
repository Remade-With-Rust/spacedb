//! Capability-gated retrieval — the AI-age access path.
//!
//! An AI agent's semantic retrieval is exactly "query embedding in → top-k out",
//! and it runs **only under an authorized capability** (M5). [`retrieve`] takes
//! the authorization [`Decision`] from `spacedb-access` and performs the search
//! only if it allows — so an un-granted agent gets nothing, and an authorized one
//! gets results, never the corpus.
//!
//! This is the concrete mechanism behind "inaccessible by default, accessible by
//! mID-gated consent": semantic access *to results*, not bulk data.

use spacedb_access::Decision;

use crate::error::{VectorError, VectorResult};
use crate::index::{Match, VectorIndex};

/// Retrieve the top-`k` matches for `query` from `index`, but only if
/// `authorization` allows (e.g. an agent presenting a `compute`-scoped
/// capability). Denied callers get [`VectorError::Denied`] and no data.
pub fn retrieve(
    index: &VectorIndex,
    query: &[f32],
    k: usize,
    authorization: &Decision,
) -> VectorResult<Vec<Match>> {
    if !authorization.is_allowed() {
        return Err(VectorError::Denied);
    }
    index.search(query, k)
}
