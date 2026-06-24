//! [`Collection<K, V>`] — the encrypted typed table.
//!
//! A `Collection` is a [`crate::Table`] whose values are sealed under a
//! per-collection DEK ([`crate::crypto`]): the engine only ever stores
//! `nonce ‖ ciphertext`, so a host that holds the bytes (a replica on a
//! stranger's machine, in later milestones) stores something it cannot read.
//!
//! The DEK is wrapped under the vault key and persisted in the reserved
//! `_dek_wrappings` table; the `Collection` caches that **ciphertext** wrapping.
//! On every row operation it fetches the vault key through the [`KeyProvider`]
//! seam and unwraps the DEK — so a vault that locks mid-session (cold-gate)
//! immediately blocks reads and writes, rather than being bypassed by a cached
//! plaintext key.
//!
//! Keys are **not** encrypted (the engine needs them in the clear to index and
//! range-scan); only values are. Key privacy, where needed, is achieved by
//! hashing the key before it reaches the store (the ADR 0005 `blake3(rp_origin)`
//! pattern) — a caller concern, not this layer's.

use std::marker::PhantomData;
use std::sync::Arc;

use serde::{de::DeserializeOwned, Serialize};
use zeroize::Zeroizing;

use crate::codec::{decode_value, encode_value, KeyDecode, KeyEncode};
use crate::crypto::{open_row, seal_row, unwrap_dek, wrap_fresh_dek, KeyProvider, WrappedDek, KEY_LEN};
use crate::engine::{Durability, KvEngine, Readable, WriteTx};
use crate::error::{StoreError, StoreResult};
use crate::table::Table;

/// The reserved table that stores each collection's wrapped DEK, keyed by
/// collection name. Collection names may not collide with reserved (`_`-prefixed)
/// tables.
pub const DEK_WRAPPINGS_TABLE: &str = "_dek_wrappings";

fn wrappings_table() -> Table<String, WrappedDek> {
    Table::new(DEK_WRAPPINGS_TABLE)
}

/// An encrypted, typed collection. Rows are AEAD-sealed under a per-collection
/// DEK; see the module docs for the trust model.
pub struct Collection<K, V> {
    name: String,
    schema_version: u32,
    /// The DEK wrapped under the vault key — ciphertext, safe to hold in memory.
    wrapped_dek: WrappedDek,
    key_provider: Arc<dyn KeyProvider>,
    _types: PhantomData<fn() -> (K, V)>,
}

impl<K, V> std::fmt::Debug for Collection<K, V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Deliberately omits the key provider and the wrapped DEK.
        f.debug_struct("Collection")
            .field("name", &self.name)
            .field("schema_version", &self.schema_version)
            .finish_non_exhaustive()
    }
}

impl<K, V> Collection<K, V>
where
    K: KeyEncode + KeyDecode,
    V: Serialize + DeserializeOwned,
{
    /// Open an **existing** collection. Errors with
    /// [`StoreError::CollectionNotFound`] if no DEK wrapping has been provisioned.
    pub fn open<E: KvEngine>(
        engine: &E,
        key_provider: Arc<dyn KeyProvider>,
        name: impl Into<String>,
        schema_version: u32,
    ) -> StoreResult<Self> {
        let name = Self::checked_name(name)?;
        let r = engine.begin_read()?;
        let wrapped = wrappings_table()
            .get(&r, &name)?
            .ok_or_else(|| StoreError::CollectionNotFound(name.clone()))?;
        Ok(Self::assemble(name, schema_version, wrapped, key_provider))
    }

    /// Open a collection, provisioning a fresh DEK on first use. The check and the
    /// create happen in one write transaction, so a collection is never
    /// double-provisioned with conflicting DEKs by a concurrent opener.
    pub fn open_or_create<E: KvEngine>(
        engine: &E,
        key_provider: Arc<dyn KeyProvider>,
        name: impl Into<String>,
        schema_version: u32,
    ) -> StoreResult<Self> {
        let name = Self::checked_name(name)?;
        let table = wrappings_table();

        let mut w = engine.begin_write(Durability::Immediate)?;
        if let Some(existing) = table.get(&w, &name)? {
            // Already provisioned — nothing to write. Drop the txn (no commit).
            drop(w);
            return Ok(Self::assemble(name, schema_version, existing, key_provider));
        }

        // First use: generate + wrap a fresh DEK under the vault key.
        let vault_key = key_provider.vault_key()?;
        let (wrapped, _dek) = wrap_fresh_dek(&vault_key, &name)?;
        table.put(&mut w, &name, &wrapped)?;
        w.commit()?;

        Ok(Self::assemble(name, schema_version, wrapped, key_provider))
    }

    fn assemble(
        name: String,
        schema_version: u32,
        wrapped_dek: WrappedDek,
        key_provider: Arc<dyn KeyProvider>,
    ) -> Self {
        Self {
            name,
            schema_version,
            wrapped_dek,
            key_provider,
            _types: PhantomData,
        }
    }

    fn checked_name(name: impl Into<String>) -> StoreResult<String> {
        let name = name.into();
        if name.starts_with('_') {
            return Err(StoreError::ReservedName(name));
        }
        Ok(name)
    }

    /// The collection's name (its table name).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The schema version bound into every row's AAD.
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Fetch the vault key (cold-gated) and unwrap this collection's DEK. Done
    /// per operation so a mid-session lock takes effect immediately.
    fn dek(&self) -> StoreResult<Zeroizing<[u8; KEY_LEN]>> {
        let vault_key = self.key_provider.vault_key()?;
        Ok(unwrap_dek(&vault_key, &self.name, &self.wrapped_dek)?)
    }

    /// Fetch and decrypt the value for `key`, or `None` if absent. A missing row
    /// returns `None` **without** touching the vault — only a present row requires
    /// an unlock to decrypt.
    pub fn get(&self, tx: &impl Readable, key: &K) -> StoreResult<Option<V>> {
        let key_bytes = key.encode();
        let sealed = match tx.get_raw(&self.name, &key_bytes)? {
            Some(bytes) => bytes,
            None => return Ok(None),
        };
        let dek = self.dek()?;
        let plain = open_row(&dek, &self.name, &key_bytes, self.schema_version, &sealed)?;
        Ok(Some(decode_value(&plain)?))
    }

    /// Encrypt and store `value` under `key`.
    pub fn put(&self, tx: &mut impl WriteTx, key: &K, value: &V) -> StoreResult<()> {
        let key_bytes = key.encode();
        let dek = self.dek()?;
        let sealed = seal_row(
            &dek,
            &self.name,
            &key_bytes,
            self.schema_version,
            &encode_value(value)?,
        )?;
        tx.put_raw(&self.name, &key_bytes, &sealed)
    }

    /// Remove `key`. Returns `true` if a value was present. No key material is
    /// needed to delete a ciphertext row.
    pub fn delete(&self, tx: &mut impl WriteTx, key: &K) -> StoreResult<bool> {
        tx.delete_raw(&self.name, &key.encode())
    }

    /// Decrypt and return the `(key, value)` pairs in `[lo, hi)`, in ascending
    /// logical key order. The DEK is unwrapped once for the whole scan.
    pub fn range(&self, tx: &impl Readable, lo: &K, hi: &K) -> StoreResult<Vec<(K, V)>> {
        let raw = tx.range_raw(&self.name, &lo.encode(), &hi.encode())?;
        if raw.is_empty() {
            return Ok(Vec::new());
        }
        let dek = self.dek()?;
        raw.into_iter()
            .map(|(key_bytes, sealed)| {
                let plain = open_row(&dek, &self.name, &key_bytes, self.schema_version, &sealed)?;
                Ok((K::decode(&key_bytes)?, decode_value(&plain)?))
            })
            .collect()
    }
}
