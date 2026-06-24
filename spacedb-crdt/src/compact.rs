//! Update-log compaction — the CRDT-side counterpart to shard reclamation.
//!
//! A log-based sync store accumulates a pile of small update blobs over a
//! document's life. Most of that history is redundant: superseded register
//! writes, GC-able deleted content. [`compact_updates`] merges a batch of v1
//! updates into a single equivalent update — same converged state, dropped
//! redundancy — so the stored log can be replaced by one compact blob.
//!
//! It is order-independent and idempotent (it's a CRDT merge), but it is only
//! *safe to discard the originals* once every replica has acknowledged the
//! frontier the merged update covers — otherwise a peer still syncing from an
//! older point would miss intermediate ops. Detecting that frontier is the
//! caller's job (via [`CrdtDoc::state_vector`](crate::CrdtDoc::state_vector)).

use yrs::merge_updates_v1;

use crate::error::{CrdtError, CrdtResult};

/// Merge a batch of v1-encoded updates into one compacted, equivalent update.
pub fn compact_updates(updates: &[Vec<u8>]) -> CrdtResult<Vec<u8>> {
    merge_updates_v1(updates.iter().map(|u| u.as_slice()))
        .map_err(|e| CrdtError::ApplyUpdate(format!("compact: {e}")))
}
