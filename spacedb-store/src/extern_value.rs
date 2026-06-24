//! Large-value externalization (the seam, stubbed).
//!
//! redb reclaims free space only past the oldest live read snapshot, so letting
//! multi-megabyte blobs sit inline in the B-tree bloats the file and slows scans.
//! The mission's rule (L0): values over a threshold are **content-addressed and
//! handed to L2** (`maestro-disco`'s chunk store), and only a small reference is
//! kept inline.
//!
//! S3 ships the *decision* and the *reference type* — the threshold, the BLAKE3
//! content hash, and the [`classify`] split into inline-vs-externalized. The
//! actual chunk handoff (and rehydration on read) is an **L2 seam**: it belongs to
//! `maestro-disco` / the operator, not to this single-node primitive, so it is
//! not wired into [`crate::Collection`] yet. This module is what that wiring will
//! call.

use serde::{Deserialize, Serialize};

/// Values larger than this (64 KiB) are externalized rather than stored inline.
pub const EXTERNALIZE_THRESHOLD: usize = 64 * 1024;

/// A reference to an externalized value: its BLAKE3 content hash and length. This
/// is what lives inline in the B-tree in place of the bytes; L2 resolves the hash
/// to the chunk(s) holding the real data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternRef {
    /// BLAKE3 hash of the externalized bytes (MATA's content-address function).
    pub hash: [u8; 32],
    /// Length of the externalized bytes.
    pub len: u64,
}

/// Where a value's bytes should live.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValuePlacement {
    /// Small enough to store directly in the B-tree.
    Inline(Vec<u8>),
    /// Too large: store only this reference inline; the bytes go to L2.
    Externalized(ExternRef),
}

/// The BLAKE3 content hash of `bytes` — the content address used to externalize
/// and later resolve a large value.
pub fn content_hash(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}

/// True if a value of this size should be externalized rather than stored inline.
pub fn should_externalize(len: usize) -> bool {
    len > EXTERNALIZE_THRESHOLD
}

/// Decide where `bytes` should live. Values at or below [`EXTERNALIZE_THRESHOLD`]
/// stay [`Inline`](ValuePlacement::Inline); larger ones become an
/// [`Externalized`](ValuePlacement::Externalized) reference (the caller is then
/// responsible for handing the bytes to L2).
pub fn classify(bytes: &[u8]) -> ValuePlacement {
    if should_externalize(bytes.len()) {
        ValuePlacement::Externalized(ExternRef {
            hash: content_hash(bytes),
            len: bytes.len() as u64,
        })
    } else {
        ValuePlacement::Inline(bytes.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{decode_value, encode_value};

    #[test]
    fn small_values_stay_inline() {
        let small = vec![7u8; EXTERNALIZE_THRESHOLD]; // exactly at threshold = inline
        assert!(matches!(classify(&small), ValuePlacement::Inline(_)));
        assert!(!should_externalize(small.len()));
    }

    #[test]
    fn large_values_are_externalized_with_hash_and_len() {
        let big = vec![7u8; EXTERNALIZE_THRESHOLD + 1];
        match classify(&big) {
            ValuePlacement::Externalized(r) => {
                assert_eq!(r.len, big.len() as u64);
                assert_eq!(r.hash, content_hash(&big));
            }
            ValuePlacement::Inline(_) => panic!("expected externalization just over the threshold"),
        }
    }

    #[test]
    fn content_hash_is_deterministic_and_content_sensitive() {
        let a = content_hash(b"hello");
        assert_eq!(a, content_hash(b"hello"));
        assert_ne!(a, content_hash(b"hellp"));
    }

    #[test]
    fn extern_ref_round_trips_through_the_value_codec() {
        let r = ExternRef {
            hash: content_hash(b"payload"),
            len: 123,
        };
        let bytes = encode_value(&r).unwrap();
        assert_eq!(decode_value::<ExternRef>(&bytes).unwrap(), r);
    }
}
