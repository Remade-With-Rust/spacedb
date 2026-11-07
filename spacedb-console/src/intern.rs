//! A per-rollup name interner: a name is hashed to an index once, the tally
//! lives at the index, and the ordered map a board exposes is built at the end.
//!
//! The rollups aggregate tens of thousands of observations over a handful of
//! repeating names. Keying a `BTreeMap<String, _>` per observation meant an
//! ordered search (string compares) and a copy of the name for every one of
//! them; round 1 measured the copy as cheaper than a lookup-then-insert, which
//! said the compares were the cost. Here each observation pays one hash of its
//! bytes and one array index.

use std::collections::{BTreeMap, HashMap};
use std::hash::{BuildHasherDefault, Hasher};

/// FNV-1a over the name's bytes. Names here are operator data (DIDs, ids), not
/// attacker-chosen keys, so a fast non-keyed hash is appropriate.
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

/// Names interned to slots of `T`.
pub(crate) struct Interner<T> {
    index: HashMap<String, usize, BuildHasherDefault<NameHasher>>,
    names: Vec<String>,
    slots: Vec<T>,
}

impl<T: Default> Interner<T> {
    pub(crate) fn new() -> Self {
        Self {
            index: HashMap::default(),
            names: Vec::new(),
            slots: Vec::new(),
        }
    }

    /// The slot for `name`, created (and the name copied) only on first sight.
    pub(crate) fn slot(&mut self, name: &str) -> &mut T {
        let at = match self.index.get(name) {
            Some(&at) => at,
            None => {
                let at = self.slots.len();
                self.index.insert(name.to_string(), at);
                self.names.push(name.to_string());
                self.slots.push(T::default());
                at
            }
        };
        &mut self.slots[at]
    }

    /// The ordered `name -> slot` map.
    pub(crate) fn into_map(self) -> BTreeMap<String, T> {
        self.names.into_iter().zip(self.slots).collect()
    }
}
