//! The SDK's name-keyed maps (collections, fields) hashed with FNV-1a.
//!
//! Every write looks a collection up twice (schema, document) and a field up
//! once. The names are the application's own schema, not attacker-chosen keys,
//! so the keyed SipHash the standard map defaults to buys nothing here and cost
//! three SipHash rounds per operation.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

/// FNV-1a over the name's bytes.
#[derive(Default)]
pub(crate) struct NameHasher(u64);

impl Hasher for NameHasher {
    fn write(&mut self, bytes: &[u8]) {
        let mut h = if self.0 == 0 { 0xcbf2_9ce4_8422_2325 } else { self.0 };
        for &b in bytes {
            h = (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
        self.0 = h;
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// A map keyed by a name.
pub(crate) type NameMap<V> = HashMap<String, V, BuildHasherDefault<NameHasher>>;
