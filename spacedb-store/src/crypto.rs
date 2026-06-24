//! The AEAD value boundary — the zero-knowledge linchpin.
//!
//! SpaceDB stores **opaque ciphertext**; the engine never sees plaintext. This
//! module owns the crypto that makes that true, mirroring the shipped, audited
//! MATA `dek_manager` envelope:
//!
//! - Each **collection** has a random 32-byte **DEK** (data encryption key) that
//!   encrypts its rows.
//! - The DEK is **wrapped** (AES-256-GCM) under the owner's **vault key**, with
//!   the collection id bound as AAD so a wrapping cannot be relocated to another
//!   collection. Rotating the passphrase/vault key **re-wraps the DEK**, never
//!   the rows ([`rewrap_dek`]).
//! - Rows are sealed under the DEK with a fresh nonce and an AAD that binds the
//!   row's **location** — `table ‖ key ‖ schema_version` — so a ciphertext can't
//!   be moved to a different key, table, or format version ([`seal_row`] /
//!   [`open_row`]). This follows the `file_id ‖ chunk_index` precedent and ADR
//!   0006 S2 ("AAD binds the field, not just the record").
//!
//! ## The open-core seam: [`KeyProvider`]
//!
//! `spacedb-store` does the wrapping itself but never *owns* the vault key — it
//! asks for it through [`KeyProvider`], a trait an operator implements. The
//! shipped [`StaticKeyProvider`] is enough for a local/self-hosted developer (who
//! supplies a key derived from their passphrase); MATA's hosted product
//! implements it against the Home Computer's cold-on-boot / warm-TTL vault
//! coordinator, so a locked vault returns [`CryptoError::Cold`] and no row can be
//! read. The key is fetched per operation precisely so that cold-gating is
//! honoured mid-session rather than bypassed by a cached key.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::error::StoreError;

/// AES-256-GCM key length (the DEK and the vault key are both this size).
pub const KEY_LEN: usize = 32;
/// AES-256-GCM nonce length.
pub const NONCE_LEN: usize = 12;
/// AES-256-GCM authentication tag length (appended to ciphertext).
pub const TAG_LEN: usize = 16;

#[derive(Debug, Error)]
pub enum CryptoError {
    /// The vault is locked — no key material is available. Callers surface this
    /// as "the user must unlock from a paired device" rather than an error.
    #[error("vault is cold; unlock required")]
    Cold,
    /// AES-GCM encryption or, more importantly, **decryption/verification**
    /// failed — corrupted ciphertext, a wrong key, or an AAD mismatch (a
    /// ciphertext presented at the wrong location).
    #[error("AEAD failure: {0}")]
    Aead(String),
    /// An unwrapped DEK was not exactly [`KEY_LEN`] bytes — a sign of corruption
    /// or a wrap produced by a different scheme.
    #[error("unwrapped DEK has wrong length: expected {KEY_LEN}, got {0}")]
    BadDekLength(usize),
    /// A sealed row was shorter than a bare nonce — truncated/corrupt.
    #[error("sealed row too short: {0} bytes")]
    ShortRow(usize),
}

impl From<CryptoError> for StoreError {
    fn from(e: CryptoError) -> Self {
        match e {
            CryptoError::Cold => StoreError::Cold,
            other => StoreError::Crypto(other.to_string()),
        }
    }
}

/// The seam through which an operator supplies the vault key.
///
/// Object-safe on purpose: a [`crate::Collection`] holds an `Arc<dyn KeyProvider>`
/// so the store is not generic over the provider. Implementations MUST return
/// [`CryptoError::Cold`] when no key is currently available, and SHOULD hand back
/// a [`Zeroizing`] copy so the key is wiped when the borrow ends.
pub trait KeyProvider: Send + Sync {
    /// The 32-byte vault key, or [`CryptoError::Cold`] if the vault is locked.
    fn vault_key(&self) -> Result<Zeroizing<[u8; KEY_LEN]>, CryptoError>;
}

/// A fixed-key provider. Suitable for a local/self-hosted developer who derives a
/// 32-byte key from their passphrase (e.g. Argon2id) and holds it for the
/// session. `cold()` models a locked vault for tests and for "no key yet" states.
#[derive(Clone)]
pub struct StaticKeyProvider {
    key: Option<[u8; KEY_LEN]>,
}

impl StaticKeyProvider {
    /// A warm provider that always yields `key`.
    pub fn new(key: [u8; KEY_LEN]) -> Self {
        Self { key: Some(key) }
    }

    /// A cold provider that always returns [`CryptoError::Cold`].
    pub fn cold() -> Self {
        Self { key: None }
    }
}

impl KeyProvider for StaticKeyProvider {
    fn vault_key(&self) -> Result<Zeroizing<[u8; KEY_LEN]>, CryptoError> {
        self.key.map(Zeroizing::new).ok_or(CryptoError::Cold)
    }
}

/// A DEK encrypted under the vault key. The collection id it is bound to is the
/// key it is stored under (in the reserved `_dek_wrappings` table), and is passed
/// explicitly as AAD to [`unwrap_dek`] — it is not stored in the struct.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WrappedDek {
    /// The nonce used to wrap the DEK.
    pub wrap_nonce: [u8; NONCE_LEN],
    /// The wrapped DEK: `AES-256-GCM(vault_key, dek, aad = collection_id)`.
    /// `KEY_LEN + TAG_LEN` bytes.
    pub wrap_ciphertext: Vec<u8>,
}

fn cipher_for(key: &[u8; KEY_LEN]) -> Aes256Gcm {
    Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key.as_slice()))
}

/// Generate a fresh random DEK and wrap it under `vault_key`, binding
/// `collection_id` as AAD. Returns the wrapping (to persist) and the raw DEK (to
/// use for the current operation, then drop — it zeroizes).
pub fn wrap_fresh_dek(
    vault_key: &[u8; KEY_LEN],
    collection_id: &str,
) -> Result<(WrappedDek, Zeroizing<[u8; KEY_LEN]>), CryptoError> {
    let mut dek = Zeroizing::new([0u8; KEY_LEN]);
    OsRng.fill_bytes(dek.as_mut());

    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);

    let ciphertext = cipher_for(vault_key)
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: dek.as_slice(),
                aad: collection_id.as_bytes(),
            },
        )
        .map_err(|e| CryptoError::Aead(format!("wrap: {e}")))?;

    Ok((
        WrappedDek {
            wrap_nonce: nonce,
            wrap_ciphertext: ciphertext,
        },
        dek,
    ))
}

/// Unwrap a DEK with `vault_key`, verifying it was bound to `collection_id`.
/// Fails with [`CryptoError::Aead`] on a wrong key, tampered bytes, or a
/// collection-id (AAD) mismatch.
pub fn unwrap_dek(
    vault_key: &[u8; KEY_LEN],
    collection_id: &str,
    wrapped: &WrappedDek,
) -> Result<Zeroizing<[u8; KEY_LEN]>, CryptoError> {
    let plaintext = Zeroizing::new(
        cipher_for(vault_key)
            .decrypt(
                Nonce::from_slice(&wrapped.wrap_nonce),
                Payload {
                    msg: &wrapped.wrap_ciphertext,
                    aad: collection_id.as_bytes(),
                },
            )
            .map_err(|e| CryptoError::Aead(format!("unwrap: {e}")))?,
    );
    if plaintext.len() != KEY_LEN {
        return Err(CryptoError::BadDekLength(plaintext.len()));
    }
    let mut dek = Zeroizing::new([0u8; KEY_LEN]);
    dek.copy_from_slice(&plaintext);
    Ok(dek)
}

/// Re-wrap an existing DEK from `old_vault_key` to `new_vault_key` — the
/// passphrase/vault-key **rotation** primitive. The DEK (and therefore every row
/// encrypted under it) is unchanged, so rotation costs one re-wrap per collection
/// rather than re-encrypting the data (ADR 0006 S3/S4).
pub fn rewrap_dek(
    old_vault_key: &[u8; KEY_LEN],
    new_vault_key: &[u8; KEY_LEN],
    collection_id: &str,
    wrapped: &WrappedDek,
) -> Result<WrappedDek, CryptoError> {
    let dek = unwrap_dek(old_vault_key, collection_id, wrapped)?;

    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    let ciphertext = cipher_for(new_vault_key)
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: dek.as_slice(),
                aad: collection_id.as_bytes(),
            },
        )
        .map_err(|e| CryptoError::Aead(format!("rewrap: {e}")))?;

    Ok(WrappedDek {
        wrap_nonce: nonce,
        wrap_ciphertext: ciphertext,
    })
}

/// The AAD that binds a row's ciphertext to its **location**:
/// `len(table) ‖ table ‖ len(key) ‖ key ‖ schema_version`. Length-prefixing makes
/// the boundary between `table` and `key` unambiguous, so no two distinct
/// locations can produce the same AAD.
fn row_aad(table: &str, key: &[u8], schema_version: u32) -> Vec<u8> {
    let mut aad = Vec::with_capacity(4 + table.len() + 4 + key.len() + 4);
    aad.extend_from_slice(&(table.len() as u32).to_be_bytes());
    aad.extend_from_slice(table.as_bytes());
    aad.extend_from_slice(&(key.len() as u32).to_be_bytes());
    aad.extend_from_slice(key);
    aad.extend_from_slice(&schema_version.to_be_bytes());
    aad
}

/// Seal a row's plaintext under the collection `dek`, binding its location. The
/// returned bytes are `nonce ‖ ciphertext` — what the engine stores.
pub fn seal_row(
    dek: &[u8; KEY_LEN],
    table: &str,
    key: &[u8],
    schema_version: u32,
    plaintext: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    let aad = row_aad(table, key, schema_version);
    let ciphertext = cipher_for(dek)
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|e| CryptoError::Aead(format!("seal: {e}")))?;
    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Open a `nonce ‖ ciphertext` row sealed by [`seal_row`], verifying it was
/// sealed at exactly this `(table, key, schema_version)`. An AAD mismatch (a row
/// presented at the wrong location) fails as [`CryptoError::Aead`].
pub fn open_row(
    dek: &[u8; KEY_LEN],
    table: &str,
    key: &[u8],
    schema_version: u32,
    sealed: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    if sealed.len() < NONCE_LEN {
        return Err(CryptoError::ShortRow(sealed.len()));
    }
    let (nonce, ciphertext) = sealed.split_at(NONCE_LEN);
    let aad = row_aad(table, key, schema_version);
    cipher_for(dek)
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad: &aad,
            },
        )
        .map_err(|e| CryptoError::Aead(format!("open: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const VK_A: [u8; KEY_LEN] = [0xAA; KEY_LEN];
    const VK_B: [u8; KEY_LEN] = [0xBB; KEY_LEN];

    // --- DEK envelope ---

    #[test]
    fn wrap_then_unwrap_yields_same_dek() {
        let (w, dek) = wrap_fresh_dek(&VK_A, "people").unwrap();
        let dek2 = unwrap_dek(&VK_A, "people", &w).unwrap();
        assert_eq!(dek.as_slice(), dek2.as_slice());
    }

    #[test]
    fn fresh_dek_is_not_all_zero() {
        let (_w, dek) = wrap_fresh_dek(&VK_A, "c").unwrap();
        assert!(dek.iter().any(|b| *b != 0));
    }

    #[test]
    fn different_collections_get_different_deks() {
        let (_w1, d1) = wrap_fresh_dek(&VK_A, "c1").unwrap();
        let (_w2, d2) = wrap_fresh_dek(&VK_A, "c2").unwrap();
        assert_ne!(d1.as_slice(), d2.as_slice());
    }

    #[test]
    fn unwrap_with_wrong_vault_key_fails() {
        let (w, _dek) = wrap_fresh_dek(&VK_A, "c").unwrap();
        let err = unwrap_dek(&VK_B, "c", &w).unwrap_err();
        assert!(matches!(err, CryptoError::Aead(_)));
    }

    #[test]
    fn unwrap_with_wrong_collection_id_fails_on_aad() {
        let (w, _dek) = wrap_fresh_dek(&VK_A, "people").unwrap();
        let err = unwrap_dek(&VK_A, "passwords", &w).unwrap_err();
        assert!(matches!(err, CryptoError::Aead(_)));
    }

    #[test]
    fn tampered_wrap_ciphertext_fails() {
        let (mut w, _dek) = wrap_fresh_dek(&VK_A, "c").unwrap();
        w.wrap_ciphertext[0] ^= 0x01;
        let err = unwrap_dek(&VK_A, "c", &w).unwrap_err();
        assert!(matches!(err, CryptoError::Aead(_)));
    }

    #[test]
    fn rewrap_rotates_vault_key_without_changing_the_dek() {
        let (w_a, dek_a) = wrap_fresh_dek(&VK_A, "c").unwrap();
        let w_b = rewrap_dek(&VK_A, &VK_B, "c", &w_a).unwrap();
        // old key no longer opens the new wrapping
        assert!(unwrap_dek(&VK_A, "c", &w_b).is_err());
        // new key opens it to the SAME dek (so existing rows still decrypt)
        let dek_b = unwrap_dek(&VK_B, "c", &w_b).unwrap();
        assert_eq!(dek_a.as_slice(), dek_b.as_slice());
    }

    // --- row seal/open ---

    #[test]
    fn seal_then_open_round_trips() {
        let dek = [0x11; KEY_LEN];
        let sealed = seal_row(&dek, "people", b"alice", 1, b"payload").unwrap();
        assert!(!sealed
            .windows(b"payload".len())
            .any(|w| w == b"payload"), "engine bytes must be ciphertext");
        let opened = open_row(&dek, "people", b"alice", 1, &sealed).unwrap();
        assert_eq!(opened, b"payload");
    }

    #[test]
    fn row_with_wrong_dek_fails() {
        let sealed = seal_row(&[0x11; KEY_LEN], "t", b"k", 1, b"v").unwrap();
        let err = open_row(&[0x22; KEY_LEN], "t", b"k", 1, &sealed).unwrap_err();
        assert!(matches!(err, CryptoError::Aead(_)));
    }

    #[test]
    fn row_relocated_to_different_key_fails_on_aad() {
        let dek = [0x11; KEY_LEN];
        let sealed = seal_row(&dek, "t", b"key1", 1, b"v").unwrap();
        assert!(open_row(&dek, "t", b"key2", 1, &sealed).is_err());
    }

    #[test]
    fn row_relocated_to_different_table_fails_on_aad() {
        let dek = [0x11; KEY_LEN];
        let sealed = seal_row(&dek, "table_a", b"k", 1, b"v").unwrap();
        assert!(open_row(&dek, "table_b", b"k", 1, &sealed).is_err());
    }

    #[test]
    fn row_with_wrong_schema_version_fails_on_aad() {
        let dek = [0x11; KEY_LEN];
        let sealed = seal_row(&dek, "t", b"k", 1, b"v").unwrap();
        assert!(open_row(&dek, "t", b"k", 2, &sealed).is_err());
    }

    #[test]
    fn tampered_row_fails() {
        let dek = [0x11; KEY_LEN];
        let mut sealed = seal_row(&dek, "t", b"k", 1, b"v").unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 0x01; // flip a tag byte
        assert!(open_row(&dek, "t", b"k", 1, &sealed).is_err());
    }

    #[test]
    fn open_short_row_is_typed_error() {
        let err = open_row(&[0; KEY_LEN], "t", b"k", 1, &[0u8; 4]).unwrap_err();
        assert!(matches!(err, CryptoError::ShortRow(4)));
    }

    // --- key provider ---

    #[test]
    fn static_provider_warm_and_cold() {
        assert_eq!(StaticKeyProvider::new(VK_A).vault_key().unwrap().as_slice(), &VK_A);
        assert!(matches!(
            StaticKeyProvider::cold().vault_key().unwrap_err(),
            CryptoError::Cold
        ));
    }
}
