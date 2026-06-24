//! The two codecs every layer above rests on: a **value codec** and an
//! **order-preserving key codec**.
//!
//! ## Value codec — `postcard`
//!
//! Values serialize through `postcard`: compact and **deterministic** (the same
//! value always produces the same bytes). Determinism is not a nicety here — in
//! later milestones encoded values feed content-addressing (L2) and re-execution
//! corroboration (L4), both of which compare bytes/hashes across machines.
//!
//! ## Key codec — order-preserving
//!
//! Keys are encoded so that **byte-lexicographic order equals logical order**:
//!
//! ```text
//!     a < b   ⟺   encode(a) < encode(b)
//! ```
//!
//! This is the single property that makes [`crate::engine::Readable::range_raw`]
//! return rows in logical key order — the basis for time-ordered audit, ledger
//! replay, and pushed-down range scans in the milestones above. It is verified
//! as a law by the proptests at the bottom of this file.
//!
//! ### How each type achieves it
//!
//! - **Fixed-width integers** (`u64`) encode as big-endian bytes — BE byte order
//!   *is* numeric order for unsigned integers.
//! - **Signed integers** (`i64`) flip the sign bit before BE encoding, so the
//!   negative range (which has the high bit set) sorts below the non-negative
//!   range.
//! - **Strings / byte strings** use an **escaped, terminated** encoding: a `0x00`
//!   content byte becomes `0x00 0x01`, and the value ends with the terminator
//!   `0x00 0x00`. Because the terminator sorts below every escaped content byte,
//!   a string that is a prefix of another sorts first (`"aa" < "aab"`) — plain
//!   concatenation would get this wrong.
//! - **Tuples** concatenate their components' encodings. This stays
//!   order-preserving **only because every component encoding is self-delimiting**
//!   (integers are fixed-width; strings are terminated), so a shorter first
//!   component can never bleed into the second.
//!
//! Each encoding is also a **bijection** — [`KeyDecode`] reverses it via a cursor
//! so composite keys can be taken apart in the same order they were built.

use serde::{de::DeserializeOwned, Serialize};

use crate::error::{StoreError, StoreResult};

// ─── Value codec ─────────────────────────────────────────────────────────────

/// Serialize a value to its canonical `postcard` bytes.
pub fn encode_value<T: Serialize>(value: &T) -> StoreResult<Vec<u8>> {
    postcard::to_allocvec(value).map_err(StoreError::value_codec)
}

/// Deserialize a value from its `postcard` bytes.
pub fn decode_value<T: DeserializeOwned>(bytes: &[u8]) -> StoreResult<T> {
    postcard::from_bytes(bytes).map_err(StoreError::value_codec)
}

// ─── Key codec ───────────────────────────────────────────────────────────────

/// A type that can be encoded into an order-preserving byte key.
///
/// Implementors MUST satisfy the ordering law `a < b ⟺ encode(a) < encode(b)`
/// and MUST produce a **self-delimiting** encoding (so tuples compose). Both
/// properties are checked by the proptests in this module for every built-in.
pub trait KeyEncode {
    /// Append this value's order-preserving encoding to `out`.
    fn encode_into(&self, out: &mut Vec<u8>);

    /// Convenience: encode into a fresh `Vec`.
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode_into(&mut out);
        out
    }
}

/// The inverse of [`KeyEncode`]: decode a value from the front of a byte cursor,
/// advancing the cursor past the bytes consumed (so tuple components decode in
/// sequence).
pub trait KeyDecode: Sized {
    /// Decode from the front of `buf`, advancing `buf` past the consumed bytes.
    fn decode_from(buf: &mut &[u8]) -> StoreResult<Self>;

    /// Decode a value that occupies the **entire** slice; errors if trailing
    /// bytes remain.
    fn decode(bytes: &[u8]) -> StoreResult<Self> {
        let mut cur = bytes;
        let value = Self::decode_from(&mut cur)?;
        if !cur.is_empty() {
            return Err(StoreError::key_decode(format!(
                "{} trailing byte(s) after key",
                cur.len()
            )));
        }
        Ok(value)
    }
}

// --- escaped, terminated byte-string encoding (the basis for strings) ---

const ESC: u8 = 0x00;
const ESC_LITERAL: u8 = 0x01; // 0x00 0x01 -> a literal 0x00 content byte
const ESC_TERM: u8 = 0x00; // 0x00 0x00 -> end of the byte string

fn encode_bytes_escaped(bytes: &[u8], out: &mut Vec<u8>) {
    for &b in bytes {
        if b == ESC {
            out.push(ESC);
            out.push(ESC_LITERAL);
        } else {
            out.push(b);
        }
    }
    out.push(ESC);
    out.push(ESC_TERM);
}

fn decode_bytes_escaped(buf: &mut &[u8]) -> StoreResult<Vec<u8>> {
    let data = *buf;
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        if b != ESC {
            out.push(b);
            i += 1;
            continue;
        }
        // b == ESC: must have a following discriminator byte.
        let next = *data
            .get(i + 1)
            .ok_or_else(|| StoreError::key_decode("truncated escape sequence in key"))?;
        match next {
            ESC_TERM => {
                *buf = &data[i + 2..];
                return Ok(out);
            }
            ESC_LITERAL => {
                out.push(0x00);
                i += 2;
            }
            other => {
                return Err(StoreError::key_decode(format!(
                    "invalid escape 0x00 0x{other:02x} in key"
                )))
            }
        }
    }
    Err(StoreError::key_decode("unterminated byte string in key"))
}

// --- u64 ---

impl KeyEncode for u64 {
    fn encode_into(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.to_be_bytes());
    }
}

impl KeyDecode for u64 {
    fn decode_from(buf: &mut &[u8]) -> StoreResult<Self> {
        if buf.len() < 8 {
            return Err(StoreError::key_decode("need 8 bytes for u64 key"));
        }
        let (head, tail) = buf.split_at(8);
        *buf = tail;
        let arr: [u8; 8] = head.try_into().expect("split_at(8) yields 8 bytes");
        Ok(u64::from_be_bytes(arr))
    }
}

// --- i64 (sign-bit-flipped big-endian: negatives sort below non-negatives) ---

const I64_SIGN_FLIP: u64 = 1 << 63;

impl KeyEncode for i64 {
    fn encode_into(&self, out: &mut Vec<u8>) {
        let biased = (*self as u64) ^ I64_SIGN_FLIP;
        out.extend_from_slice(&biased.to_be_bytes());
    }
}

impl KeyDecode for i64 {
    fn decode_from(buf: &mut &[u8]) -> StoreResult<Self> {
        if buf.len() < 8 {
            return Err(StoreError::key_decode("need 8 bytes for i64 key"));
        }
        let (head, tail) = buf.split_at(8);
        *buf = tail;
        let arr: [u8; 8] = head.try_into().expect("split_at(8) yields 8 bytes");
        Ok((u64::from_be_bytes(arr) ^ I64_SIGN_FLIP) as i64)
    }
}

// --- String / str ---

impl KeyEncode for String {
    fn encode_into(&self, out: &mut Vec<u8>) {
        encode_bytes_escaped(self.as_bytes(), out);
    }
}

impl KeyEncode for str {
    fn encode_into(&self, out: &mut Vec<u8>) {
        encode_bytes_escaped(self.as_bytes(), out);
    }
}

impl KeyEncode for &str {
    fn encode_into(&self, out: &mut Vec<u8>) {
        encode_bytes_escaped(self.as_bytes(), out);
    }
}

impl KeyDecode for String {
    fn decode_from(buf: &mut &[u8]) -> StoreResult<Self> {
        let bytes = decode_bytes_escaped(buf)?;
        String::from_utf8(bytes).map_err(|e| StoreError::key_decode(format!("key not utf-8: {e}")))
    }
}

// --- tuples (self-delimiting components compose) ---

impl<A: KeyEncode, B: KeyEncode> KeyEncode for (A, B) {
    fn encode_into(&self, out: &mut Vec<u8>) {
        self.0.encode_into(out);
        self.1.encode_into(out);
    }
}

impl<A: KeyDecode, B: KeyDecode> KeyDecode for (A, B) {
    fn decode_from(buf: &mut &[u8]) -> StoreResult<Self> {
        let a = A::decode_from(buf)?;
        let b = B::decode_from(buf)?;
        Ok((a, b))
    }
}

impl<A: KeyEncode, B: KeyEncode, C: KeyEncode> KeyEncode for (A, B, C) {
    fn encode_into(&self, out: &mut Vec<u8>) {
        self.0.encode_into(out);
        self.1.encode_into(out);
        self.2.encode_into(out);
    }
}

impl<A: KeyDecode, B: KeyDecode, C: KeyDecode> KeyDecode for (A, B, C) {
    fn decode_from(buf: &mut &[u8]) -> StoreResult<Self> {
        let a = A::decode_from(buf)?;
        let b = B::decode_from(buf)?;
        let c = C::decode_from(buf)?;
        Ok((a, b, c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // --- value codec ---

    #[test]
    fn value_codec_round_trips_and_is_deterministic() {
        #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        struct V {
            a: u32,
            b: String,
            c: Vec<u8>,
        }
        let v = V {
            a: 7,
            b: "hello".into(),
            c: vec![1, 2, 3],
        };
        let e1 = encode_value(&v).unwrap();
        let e2 = encode_value(&v).unwrap();
        assert_eq!(e1, e2, "postcard must be deterministic");
        assert_eq!(decode_value::<V>(&e1).unwrap(), v);
    }

    // --- key codec: explicit tricky cases for the ordering law ---

    fn enc<K: KeyEncode>(k: &K) -> Vec<u8> {
        k.encode()
    }

    #[test]
    fn string_prefix_sorts_before_extension() {
        // "aa" is a prefix of "aab"; the terminator must make it sort first.
        assert!(enc(&"aa".to_string()) < enc(&"aab".to_string()));
        assert!(enc(&"aa".to_string()) < enc(&"ab".to_string()));
        assert!(enc(&"".to_string()) < enc(&"a".to_string()));
    }

    #[test]
    fn string_with_embedded_null_round_trips_and_orders() {
        let with_null = String::from_utf8(vec![b'a', 0x00, b'b']).unwrap();
        let mut buf = enc(&with_null);
        // round-trip
        assert_eq!(String::decode(&buf).unwrap(), with_null);
        // the 0x00 must be escaped, never appear as a bare terminator mid-value
        buf.clear();
        with_null.encode_into(&mut buf);
        assert!(buf.windows(2).filter(|w| *w == [0x00, 0x00]).count() == 1,
            "only the terminator may be 0x00 0x00");
    }

    #[test]
    fn i64_negatives_sort_below_non_negatives() {
        assert!(enc(&-1i64) < enc(&0i64));
        assert!(enc(&i64::MIN) < enc(&i64::MAX));
        assert!(enc(&-5i64) < enc(&-1i64));
    }

    #[test]
    fn tuple_orders_by_first_then_second_component() {
        assert!(enc(&(1u64, "z".to_string())) < enc(&(2u64, "a".to_string())));
        assert!(enc(&(2u64, "a".to_string())) < enc(&(2u64, "b".to_string())));
        // a longer first string component must not bleed into the second
        assert!(enc(&("a".to_string(), "z".to_string())) < enc(&("ab".to_string(), "a".to_string())));
    }

    // --- key codec: the ordering law + bijection, fuzzed ---

    /// The encoding is order-preserving iff the byte comparison of two encodings
    /// equals the logical comparison of the values.
    fn assert_order_law<K: KeyEncode + Ord>(a: &K, b: &K) {
        assert_eq!(
            a.cmp(b),
            enc(a).cmp(&enc(b)),
            "encoding must preserve order"
        );
    }

    proptest! {
        #[test]
        fn u64_round_trips(x in any::<u64>()) {
            prop_assert_eq!(u64::decode(&x.encode()).unwrap(), x);
        }

        #[test]
        fn i64_round_trips(x in any::<i64>()) {
            prop_assert_eq!(i64::decode(&x.encode()).unwrap(), x);
        }

        #[test]
        fn string_round_trips(s in any::<String>()) {
            prop_assert_eq!(String::decode(&s.encode()).unwrap(), s);
        }

        #[test]
        fn u64_order_preserving(a in any::<u64>(), b in any::<u64>()) {
            assert_order_law(&a, &b);
        }

        #[test]
        fn i64_order_preserving(a in any::<i64>(), b in any::<i64>()) {
            assert_order_law(&a, &b);
        }

        #[test]
        fn string_order_preserving(a in any::<String>(), b in any::<String>()) {
            assert_order_law(&a, &b);
        }

        #[test]
        fn tuple_u64_string_order_preserving(
            a in any::<(u64, String)>(),
            b in any::<(u64, String)>(),
        ) {
            assert_order_law(&a, &b);
        }

        #[test]
        fn tuple_string_string_round_trips_and_orders(
            a in any::<(String, String)>(),
            b in any::<(String, String)>(),
        ) {
            prop_assert_eq!(<(String, String)>::decode(&a.encode()).unwrap(), a.clone());
            assert_order_law(&a, &b);
        }
    }
}
