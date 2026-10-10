//! The two-message anti-entropy wire protocol.
//!
//! Replicas reconcile by exchanging exactly two kinds of frame:
//!
//! - **`StateVector`** — "here is my per-actor version frontier; send me what I'm
//!   missing." A peer answers with the delta the sender lacks.
//! - **`Update`** — "here are CRDT updates to merge."
//!
//! Framing is a single tag byte followed by the payload, so a [`Transport`] only
//! ever moves opaque `Vec<u8>` frames (exactly what a real iroh/relay byte pipe
//! provides).
//!
//! [`Transport`]: crate::Transport

use spacedb_crdt::CrdtDoc;

use crate::error::{ReplicaError, ReplicaResult};

const TAG_STATE_VECTOR: u8 = 0;
const TAG_UPDATE: u8 = 1;

/// A frame exchanged between replicas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncMessage {
    /// A v1-encoded state vector (the sender's frontier).
    StateVector(Vec<u8>),
    /// A v1-encoded CRDT update (a delta to merge).
    Update(Vec<u8>),
}

impl SyncMessage {
    /// Serialize to a `tag ‖ payload` frame.
    pub fn encode(&self) -> Vec<u8> {
        let (tag, payload) = match self {
            SyncMessage::StateVector(sv) => (TAG_STATE_VECTOR, sv),
            SyncMessage::Update(u) => (TAG_UPDATE, u),
        };
        let mut frame = Vec::with_capacity(1 + payload.len());
        frame.push(tag);
        frame.extend_from_slice(payload);
        frame
    }

    /// [`encode`](Self::encode), consuming the message: the frame is built in
    /// the payload's own buffer when it has spare capacity (a one-byte shift,
    /// no allocation), which an encoder's output almost always has. Same bytes.
    pub fn into_frame(self) -> Vec<u8> {
        let (tag, mut payload) = match self {
            SyncMessage::StateVector(sv) => (TAG_STATE_VECTOR, sv),
            SyncMessage::Update(u) => (TAG_UPDATE, u),
        };
        if payload.len() < payload.capacity() {
            payload.insert(0, tag);
            return payload;
        }
        let mut frame = Vec::with_capacity(1 + payload.len());
        frame.push(tag);
        frame.extend_from_slice(&payload);
        frame
    }

    /// `SyncMessage::StateVector(doc.state_vector()).into_frame()`, built in one
    /// allocation: the vector is copied into the frame once, not copied out of
    /// the document and then again into the frame. Same bytes.
    pub fn state_vector_frame(doc: &CrdtDoc) -> Vec<u8> {
        doc.tagged_state_vector(TAG_STATE_VECTOR)
    }

    /// Parse a `tag ‖ payload` frame.
    pub fn decode(frame: &[u8]) -> ReplicaResult<Self> {
        Ok(match Self::decode_ref(frame)? {
            SyncFrame::StateVector(sv) => SyncMessage::StateVector(sv.to_vec()),
            SyncFrame::Update(u) => SyncMessage::Update(u.to_vec()),
        })
    }

    /// Parse a `tag ‖ payload` frame without copying the payload out of it —
    /// for a receiver that only reads the payload (answers a state vector,
    /// merges an update), which is every receiver on the sync path.
    pub fn decode_ref(frame: &[u8]) -> ReplicaResult<SyncFrame<'_>> {
        match frame.split_first() {
            Some((&TAG_STATE_VECTOR, rest)) => Ok(SyncFrame::StateVector(rest)),
            Some((&TAG_UPDATE, rest)) => Ok(SyncFrame::Update(rest)),
            _ => Err(ReplicaError::MalformedFrame),
        }
    }
}

/// A [`SyncMessage`] whose payload borrows from the frame it was parsed from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncFrame<'a> {
    /// A v1-encoded state vector.
    StateVector(&'a [u8]),
    /// A v1-encoded CRDT update.
    Update(&'a [u8]),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        for msg in [
            SyncMessage::StateVector(vec![1, 2, 3]),
            SyncMessage::Update(vec![9, 8, 7, 6]),
            SyncMessage::StateVector(vec![]),
        ] {
            assert_eq!(SyncMessage::decode(&msg.encode()).unwrap(), msg);
        }
    }

    #[test]
    fn into_frame_matches_encode_with_or_without_spare_capacity() {
        let mut roomy = Vec::with_capacity(64);
        roomy.extend_from_slice(&[4, 5, 6]);
        for msg in [
            SyncMessage::StateVector(vec![1, 2, 3]),
            SyncMessage::Update(roomy),
            SyncMessage::Update(Vec::new()),
        ] {
            assert_eq!(msg.clone().into_frame(), msg.encode());
        }
    }

    #[test]
    fn state_vector_frame_matches_encode() {
        let doc = CrdtDoc::new(1);
        let empty = SyncMessage::StateVector(doc.state_vector()).encode();
        assert_eq!(SyncMessage::state_vector_frame(&doc), empty);
        doc.increment("n", 1);
        let after = SyncMessage::StateVector(doc.state_vector()).encode();
        assert_ne!(after, empty);
        // twice: the second is served from the document's cached vector
        assert_eq!(SyncMessage::state_vector_frame(&doc), after);
        assert_eq!(SyncMessage::state_vector_frame(&doc), after);
    }

    #[test]
    fn empty_frame_is_malformed() {
        assert!(matches!(
            SyncMessage::decode(&[]),
            Err(ReplicaError::MalformedFrame)
        ));
    }
}
