//! Transparent per-row compression **inside** the AEAD boundary.
//!
//! The law is *compress, then encrypt*: ciphertext is incompressible, so the
//! only place compression can work is on the plaintext, before [`crate::crypto::seal_row`].
//! A prefixed collection therefore seals `format_byte ‖ payload`:
//!
//! - [`FORMAT_RAW`] (`0x00`) — payload is the encoded value, verbatim;
//! - [`FORMAT_ZSTD`] (`0x01`) — payload is an RFC 8878 zstd frame of it;
//! - `0x02` is **reserved** for dictionary frames (a dictionary is part of the
//!   on-disk format and must be stored, versioned and re-wrapped like a key —
//!   deliberately not shipped in the first cut).
//!
//! The format byte rides inside the sealed plaintext, so it is authenticated by
//! the AEAD and invisible to whoever holds the ciphertext. Compression is
//! attempted only above a floor ([`Compression::min_len`]) and kept only when
//! it actually shrank the payload — incompressible rows store raw at a cost of
//! one byte.
//!
//! **The length side-channel rule (normative, from the deploy plan W1.2):**
//! compression leaks plaintext redundancy through ciphertext *length*. Never
//! mix user-secret and attacker-influenced data in one compressed row; a
//! collection whose rows do so must opt out with [`Compression::Off`].

use crate::error::{StoreError, StoreResult};

/// Format byte: the payload is the encoded value, verbatim.
pub const FORMAT_RAW: u8 = 0x00;
/// Format byte: the payload is a zstd frame of the encoded value.
pub const FORMAT_ZSTD: u8 = 0x01;

/// The default zstd level. Level 3 is the ratio/CPU knee for small structured
/// rows; raise it only with a measurement.
pub const DEFAULT_COMPRESSION_LEVEL: i32 = 3;

/// The default floor below which compression is not attempted. Set from the
/// floor sweep in `examples/seal_ab.rs` (2026-08-28, structured-row corpus,
/// floor disabled): at ≤64 B compression never won (every row fell back raw);
/// at 96 B marginal wins appeared (−0.0%); at 128 B real wins began (−5.6%),
/// growing monotonically (−23.5% at 256 B, −59.4% at 512 B). The floor's only
/// cost is a wasted compress attempt — the strictly-smaller check already
/// protects size — so it sits where wins *start existing*, not where they get
/// big. Re-derive with the sweep, don't hand-tune.
pub const DEFAULT_MIN_LEN: usize = 96;

/// Per-collection write-side compression policy. Reads always honor the
/// per-row format byte regardless of policy, so flipping the policy never
/// strands existing rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    /// Compress rows of at least `min_len` encoded bytes at `level`, keeping
    /// the result only when it is strictly smaller than storing raw.
    On {
        /// zstd level (see [`DEFAULT_COMPRESSION_LEVEL`]).
        level: i32,
        /// Encoded-value floor below which compression is not attempted.
        min_len: usize,
    },
    /// Never compress. Required for collections whose rows mix user-secret and
    /// attacker-influenced bytes (the length side-channel rule above).
    Off,
}

impl Default for Compression {
    fn default() -> Self {
        Compression::On {
            level: DEFAULT_COMPRESSION_LEVEL,
            min_len: DEFAULT_MIN_LEN,
        }
    }
}

/// Wrap an encoded value in the prefixed format, compressing when the policy
/// says to and it pays. Infallible by design: a compression failure falls back
/// to raw — a put must never fail because a compressor declined.
pub(crate) fn pack_value(plain: &[u8], policy: Compression) -> Vec<u8> {
    if let Compression::On { level, min_len } = policy {
        if plain.len() >= min_len {
            if let Ok(z) = rusty_zstd::compress(plain, level) {
                if z.len() < plain.len() {
                    let mut out = Vec::with_capacity(1 + z.len());
                    out.push(FORMAT_ZSTD);
                    out.extend_from_slice(&z);
                    return out;
                }
            }
        }
    }
    let mut out = Vec::with_capacity(1 + plain.len());
    out.push(FORMAT_RAW);
    out.extend_from_slice(plain);
    out
}

/// Unwrap a prefixed payload back to the encoded value. The input is
/// AEAD-authenticated (it came out of `open_row`), so a bad format byte or a
/// broken frame is corruption or a version mix-up, not attacker data — it
/// fails loudly rather than decoding garbage.
pub(crate) fn unpack_value(packed: &[u8]) -> StoreResult<Vec<u8>> {
    match packed.split_first() {
        Some((&FORMAT_RAW, rest)) => Ok(rest.to_vec()),
        Some((&FORMAT_ZSTD, rest)) => rusty_zstd::decompress(rest)
            .map_err(|e| StoreError::Compression(format!("zstd frame: {e:?}"))),
        Some((&byte, _)) => Err(StoreError::Compression(format!(
            "unknown row format byte {byte:#04x}"
        ))),
        None => Err(StoreError::Compression("empty sealed payload".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_round_trips() {
        let plain = b"hello world";
        let packed = pack_value(plain, Compression::Off);
        assert_eq!(packed[0], FORMAT_RAW);
        assert_eq!(unpack_value(&packed).unwrap(), plain);
    }

    #[test]
    fn compressible_payload_shrinks_and_round_trips() {
        let plain = vec![b'a'; 4096];
        let packed = pack_value(&plain, Compression::default());
        assert_eq!(packed[0], FORMAT_ZSTD);
        assert!(packed.len() < plain.len());
        assert_eq!(unpack_value(&packed).unwrap(), plain);
    }

    #[test]
    fn incompressible_payload_stays_raw() {
        // Deterministic pseudo-random bytes do not compress; the policy must
        // fall back to raw rather than storing an expanded frame.
        let mut state = 0x9E3779B97F4A7C15u64;
        let plain: Vec<u8> = (0..4096)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 56) as u8
            })
            .collect();
        let packed = pack_value(&plain, Compression::default());
        assert_eq!(packed[0], FORMAT_RAW);
        assert_eq!(packed.len(), plain.len() + 1);
        assert_eq!(unpack_value(&packed).unwrap(), plain);
    }

    #[test]
    fn below_the_floor_is_not_attempted() {
        let plain = vec![b'a'; 8];
        let packed = pack_value(
            &plain,
            Compression::On {
                level: DEFAULT_COMPRESSION_LEVEL,
                min_len: 64,
            },
        );
        assert_eq!(packed[0], FORMAT_RAW);
    }

    #[test]
    fn unknown_format_byte_fails_loudly() {
        assert!(unpack_value(&[0x7F, 1, 2, 3]).is_err());
        assert!(unpack_value(&[]).is_err());
    }
}
