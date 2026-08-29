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
use crate::compress::{pack_value, unpack_value, Compression};
use crate::crypto::{open_row, seal_row, unwrap_dek, wrap_fresh_dek, KeyProvider, WrappedDek, KEY_LEN};
use crate::engine::{Durability, KvEngine, Readable, WriteTx};
use crate::error::{StoreError, StoreResult};
use crate::table::Table;

/// The reserved table that stores each collection's wrapped DEK, keyed by
/// collection name. Collection names may not collide with reserved (`_`-prefixed)
/// tables.
pub const DEK_WRAPPINGS_TABLE: &str = "_dek_wrappings";

/// The reserved table recording each collection's sealed-value format, keyed by
/// collection name. **Absent entry = legacy**: the sealed plaintext is the
/// encoded value verbatim (every pre-v2 collection). Value `2` = prefixed: the
/// sealed plaintext is `format_byte ‖ payload` ([`crate::compress`]). The entry
/// is plaintext metadata; tampering with it can only make rows fail to decode
/// (an attacker with engine write access can already destroy ciphertext), never
/// leak or forge a value — decode failures are loud ([`StoreError::Compression`]).
pub const COLLECTION_FORMATS_TABLE: &str = "_collection_formats";

/// The `_collection_formats` value for a prefixed collection.
const VALUE_FORMAT_PREFIXED: u8 = 2;

/// How a collection's sealed plaintexts are laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ValueFormat {
    /// Pre-v2: the sealed plaintext is the encoded value, verbatim.
    Legacy,
    /// v2+: the sealed plaintext is `format_byte ‖ payload`; rows may be
    /// zstd-compressed per the collection's write [`Compression`] policy.
    Prefixed,
}

fn wrappings_table() -> Table<String, WrappedDek> {
    Table::new(DEK_WRAPPINGS_TABLE)
}

fn formats_table() -> Table<String, u8> {
    Table::new(COLLECTION_FORMATS_TABLE)
}

fn read_format(tx: &impl Readable, name: &str) -> StoreResult<ValueFormat> {
    Ok(match formats_table().get(tx, &name.to_string())? {
        Some(VALUE_FORMAT_PREFIXED) => ValueFormat::Prefixed,
        // An unknown format number would mean a newer binary wrote this
        // collection — but that binary also bumped `STORE_FORMAT_VERSION`, so
        // the `_meta` gate refuses the whole store before we get here. Treat
        // anything else as legacy rather than inventing a second gate.
        _ => ValueFormat::Legacy,
    })
}

/// An encrypted, typed collection. Rows are AEAD-sealed under a per-collection
/// DEK; see the module docs for the trust model.
pub struct Collection<K, V> {
    name: String,
    schema_version: u32,
    /// The DEK wrapped under the vault key — ciphertext, safe to hold in memory.
    wrapped_dek: WrappedDek,
    key_provider: Arc<dyn KeyProvider>,
    /// How this collection's sealed plaintexts are laid out (fixed at creation).
    format: ValueFormat,
    /// Write-side compression policy; reads always honor the per-row format
    /// byte, so this can differ between openers without stranding rows.
    compression: Compression,
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
    /// Open an **existing** collection with the default write [`Compression`].
    /// Errors with [`StoreError::CollectionNotFound`] if no DEK wrapping has
    /// been provisioned. The collection's sealed-value format (legacy vs
    /// prefixed) was fixed at creation and is read from `_collection_formats`.
    pub fn open<E: KvEngine>(
        engine: &E,
        key_provider: Arc<dyn KeyProvider>,
        name: impl Into<String>,
        schema_version: u32,
    ) -> StoreResult<Self> {
        Self::open_with(engine, key_provider, name, schema_version, Compression::default())
    }

    /// [`Collection::open`] with an explicit write [`Compression`] policy.
    /// The policy affects only what this handle writes (and only on prefixed
    /// collections); reads always honor each row's own format byte.
    pub fn open_with<E: KvEngine>(
        engine: &E,
        key_provider: Arc<dyn KeyProvider>,
        name: impl Into<String>,
        schema_version: u32,
        compression: Compression,
    ) -> StoreResult<Self> {
        let name = Self::checked_name(name)?;
        let r = engine.begin_read()?;
        let wrapped = wrappings_table()
            .get(&r, &name)?
            .ok_or_else(|| StoreError::CollectionNotFound(name.clone()))?;
        let format = read_format(&r, &name)?;
        Ok(Self::assemble(name, schema_version, wrapped, key_provider, format, compression))
    }

    /// Open a collection with the default write [`Compression`], provisioning a
    /// fresh DEK on first use. The check and the create happen in one write
    /// transaction, so a collection is never double-provisioned with
    /// conflicting DEKs by a concurrent opener. A collection **created** here
    /// is prefixed (v2): its rows may compress. An existing collection keeps
    /// the format it was created with.
    pub fn open_or_create<E: KvEngine>(
        engine: &E,
        key_provider: Arc<dyn KeyProvider>,
        name: impl Into<String>,
        schema_version: u32,
    ) -> StoreResult<Self> {
        Self::open_or_create_with(engine, key_provider, name, schema_version, Compression::default())
    }

    /// [`Collection::open_or_create`] with an explicit write [`Compression`]
    /// policy. Use [`Compression::Off`] for collections whose rows mix
    /// user-secret and attacker-influenced bytes (the length side-channel rule
    /// in [`crate::compress`]).
    pub fn open_or_create_with<E: KvEngine>(
        engine: &E,
        key_provider: Arc<dyn KeyProvider>,
        name: impl Into<String>,
        schema_version: u32,
        compression: Compression,
    ) -> StoreResult<Self> {
        let name = Self::checked_name(name)?;
        let table = wrappings_table();

        let mut w = engine.begin_write(Durability::Immediate)?;
        if let Some(existing) = table.get(&w, &name)? {
            // Already provisioned — nothing to write. Drop the txn (no commit).
            let format = read_format(&w, &name)?;
            drop(w);
            return Ok(Self::assemble(name, schema_version, existing, key_provider, format, compression));
        }

        // First use: generate + wrap a fresh DEK under the vault key, and stamp
        // the collection as prefixed — same transaction, so a crash can't leave
        // a collection whose format is ambiguous.
        let vault_key = key_provider.vault_key()?;
        let (wrapped, _dek) = wrap_fresh_dek(&vault_key, &name)?;
        table.put(&mut w, &name, &wrapped)?;
        formats_table().put(&mut w, &name, &VALUE_FORMAT_PREFIXED)?;
        w.commit()?;

        Ok(Self::assemble(
            name,
            schema_version,
            wrapped,
            key_provider,
            ValueFormat::Prefixed,
            compression,
        ))
    }

    fn assemble(
        name: String,
        schema_version: u32,
        wrapped_dek: WrappedDek,
        key_provider: Arc<dyn KeyProvider>,
        format: ValueFormat,
        compression: Compression,
    ) -> Self {
        Self {
            name,
            schema_version,
            wrapped_dek,
            key_provider,
            format,
            compression,
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

    /// Encode a value into the plaintext this collection seals — verbatim for
    /// legacy collections, `format_byte ‖ payload` (compressing per policy) for
    /// prefixed ones.
    fn plaintext_for_store(&self, value: &V) -> StoreResult<Vec<u8>> {
        let plain = encode_value(value)?;
        Ok(match self.format {
            ValueFormat::Legacy => plain,
            ValueFormat::Prefixed => pack_value(&plain, self.compression),
        })
    }

    /// Decode a sealed-and-opened plaintext back into a value.
    fn value_from_plaintext(&self, plain: &[u8]) -> StoreResult<V> {
        match self.format {
            ValueFormat::Legacy => decode_value(plain),
            ValueFormat::Prefixed => decode_value(&unpack_value(plain)?),
        }
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
        Ok(Some(self.value_from_plaintext(&plain)?))
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
            &self.plaintext_for_store(value)?,
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
                Ok((K::decode(&key_bytes)?, self.value_from_plaintext(&plain)?))
            })
            .collect()
    }
}
