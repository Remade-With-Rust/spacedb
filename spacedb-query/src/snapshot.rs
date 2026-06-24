//! A pinned, content-addressed view of a shard's data.
//!
//! A query must read a *consistent* view even as the underlying CRDT keeps
//! converging, so it runs against a **pinned snapshot**: the materialized bytes of
//! a shard at a specific **frontier** (the CRDT state vector), content-addressed
//! so the query's attestation binds to exactly the data it saw.
//!
//! The snapshot bytes are opaque to this crate — how a document materializes into
//! query-readable bytes is the caller's schema choice. A CRDT-native pin is just
//! `Snapshot::pin(doc.encode_full(), doc.state_vector())`.

/// A frozen view of one shard's data at a frontier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    bytes: Vec<u8>,
    frontier: Vec<u8>,
    hash: [u8; 32],
}

impl Snapshot {
    /// Pin `bytes` at `frontier` (e.g. a CRDT state vector). The content hash is
    /// computed so the snapshot is verifiably the exact data a query ran on.
    pub fn pin(bytes: Vec<u8>, frontier: Vec<u8>) -> Self {
        let hash = *blake3::hash(&bytes).as_bytes();
        Self {
            bytes,
            frontier,
            hash,
        }
    }

    /// The materialized data the query reads.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The consistency frontier (state vector) this snapshot was pinned at.
    pub fn frontier(&self) -> &[u8] {
        &self.frontier
    }

    /// The BLAKE3 content hash of the snapshot bytes.
    pub fn hash(&self) -> [u8; 32] {
        self.hash
    }
}
