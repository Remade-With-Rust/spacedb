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

    /// Parse a `tag ‖ payload` frame.
    pub fn decode(frame: &[u8]) -> ReplicaResult<Self> {
        match frame.split_first() {
            Some((&TAG_STATE_VECTOR, rest)) => Ok(SyncMessage::StateVector(rest.to_vec())),
            Some((&TAG_UPDATE, rest)) => Ok(SyncMessage::Update(rest.to_vec())),
            _ => Err(ReplicaError::MalformedFrame),
        }
    }
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
    fn empty_frame_is_malformed() {
        assert!(matches!(
            SyncMessage::decode(&[]),
            Err(ReplicaError::MalformedFrame)
        ));
    }
}
