//! [`Table<K, V>`] — the typed primitive every layer above uses.
//!
//! A `Table` binds a table name to a key type `K` and value type `V`, and applies
//! the two codecs ([`crate::codec`]) once so nothing above has to think about
//! bytes: keys go through the order-preserving key encoding (so `range` returns
//! logical order), values through the deterministic `postcard` codec.
//!
//! It is also the seam where the **AEAD value boundary** will live (S2): `put`
//! will encrypt `V`'s bytes under the collection DEK before they reach the engine,
//! and `get`/`range` will decrypt — so the engine only ever stores ciphertext.
//! In S1 there is no crypto yet; values are stored as plaintext `postcard` bytes.

use std::marker::PhantomData;

use crate::codec::{decode_value, encode_value, KeyDecode, KeyEncode};
use crate::engine::{Readable, WriteTx};
use crate::error::StoreResult;

/// A typed handle to one table. Cheap to construct and clone; holds only the
/// table name and the `K`/`V` type binding.
#[derive(Clone, Debug)]
pub struct Table<K, V> {
    name: String,
    _types: PhantomData<fn() -> (K, V)>,
}

impl<K, V> Table<K, V>
where
    K: KeyEncode + KeyDecode,
    V: serde::Serialize + serde::de::DeserializeOwned,
{
    /// Bind a typed table to `name`.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            _types: PhantomData,
        }
    }

    /// The underlying table name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Fetch the value for `key`, or `None` if absent. Accepts any [`Readable`],
    /// so it reads from a read transaction or a write transaction's own
    /// uncommitted state.
    pub fn get(&self, tx: &impl Readable, key: &K) -> StoreResult<Option<V>> {
        match tx.get_raw(&self.name, &key.encode())? {
            Some(bytes) => Ok(Some(decode_value(&bytes)?)),
            None => Ok(None),
        }
    }

    /// Insert or overwrite `key` → `value`.
    pub fn put(&self, tx: &mut impl WriteTx, key: &K, value: &V) -> StoreResult<()> {
        tx.put_raw(&self.name, &key.encode(), &encode_value(value)?)
    }

    /// Remove `key`. Returns `true` if a value was present.
    pub fn delete(&self, tx: &mut impl WriteTx, key: &K) -> StoreResult<bool> {
        tx.delete_raw(&self.name, &key.encode())
    }

    /// Return the decoded `(key, value)` pairs in the **half-open** range
    /// `[lo, hi)`, in ascending logical key order.
    pub fn range(&self, tx: &impl Readable, lo: &K, hi: &K) -> StoreResult<Vec<(K, V)>> {
        let raw = tx.range_raw(&self.name, &lo.encode(), &hi.encode())?;
        raw.into_iter()
            .map(|(k, v)| Ok((K::decode(&k)?, decode_value(&v)?)))
            .collect()
    }
}
