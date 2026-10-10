//! Identities and signing — ECDSA P-256 (ES256), the curve mID uses.
//!
//! A [`Did`] is an opaque identifier (e.g. `did:mata:…`, `did:agent:…`); its
//! published verification key is resolved through the [`KeyDirectory`] seam, so
//! MATA can map `did:mata` via IAMHUMAN while a self-hoster uses the in-memory
//! directory. An [`Identity`] is a keypair: it signs (capabilities, sub-grants)
//! and publishes its SEC1 public key.
//!
//! [`KeyDirectory`]: crate::KeyDirectory

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use p256::ecdsa::signature::{Signer, Verifier};
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::error::{AccessError, AccessResult};

/// An identity reference: who an issuer/bearer is. Resolved to a key via the
/// directory.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Did(pub String);

impl From<&str> for Did {
    fn from(s: &str) -> Self {
        Did(s.to_string())
    }
}

impl From<String> for Did {
    fn from(s: String) -> Self {
        Did(s)
    }
}

impl std::fmt::Display for Did {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A P-256 keypair bound to a [`Did`]. Signs capabilities and sub-grants; its
/// public key is published to a directory for verifiers.
pub struct Identity {
    did: Did,
    signing_key: SigningKey,
    public_sec1: Vec<u8>,
}

impl Identity {
    /// Generate a fresh keypair for `did` using OS randomness.
    pub fn generate(did: impl Into<Did>) -> AccessResult<Self> {
        let mut raw = [0u8; 32];
        getrandom::fill(&mut raw).map_err(|e| AccessError::KeyGen(e.to_string()))?;
        let signing_key = SigningKey::from_bytes((&raw).into())
            .map_err(|e| AccessError::KeyGen(e.to_string()))?;
        let public_sec1 = signing_key
            .verifying_key()
            .to_encoded_point(true)
            .as_bytes()
            .to_vec();
        Ok(Self {
            did: did.into(),
            signing_key,
            public_sec1,
        })
    }

    /// This identity's DID.
    pub fn did(&self) -> &Did {
        &self.did
    }

    /// This identity's published SEC1 public key (compressed, 33 bytes).
    pub fn public_key(&self) -> &[u8] {
        &self.public_sec1
    }

    /// Sign `message`, returning a DER-encoded ECDSA signature.
    pub fn sign(&self, message: &[u8]) -> Vec<u8> {
        let signature: Signature = self.signing_key.sign(message);
        signature.to_der().as_bytes().to_vec()
    }
}

/// Verify a DER signature `sig_der` over `message` against a SEC1 public key.
/// Returns `false` (not an error) on any parse or verification failure — a bad
/// signature is a [`Deny`](crate::Decision), not a system error.
pub(crate) fn verify_sec1(public_sec1: &[u8], message: &[u8], sig_der: &[u8]) -> bool {
    let id = verification_id(public_sec1, blake3::hash(message).as_bytes(), sig_der);
    if already_verified(&id) {
        return true;
    }
    let ok = match parse_sec1(public_sec1) {
        Some(key) => verify_with_key(&key, message, sig_der),
        None => false,
    };
    if ok {
        remember_verified(id);
    }
    ok
}

// ─── verified-signature cache ────────────────────────────────────────────────
//
// Authorization re-verifies the same capability signatures on every request,
// and an audit log re-verifies every entry on every walk. A P-256 verify is a
// pair of scalar multiplications (~5M instructions); the cache check is one
// BLAKE3 of a few hundred bytes. The identity binds all three inputs, so only a
// BLAKE3 collision could make a different (key, message, signature) look
// verified. Only successes are kept: a bad signature is re-checked, and costs
// the same, every time. Bounded, and cleared when full.

const VERIFIED_CAP: usize = 4096;

fn verified() -> &'static Mutex<HashSet<[u8; 32]>> {
    static VERIFIED: OnceLock<Mutex<HashSet<[u8; 32]>>> = OnceLock::new();
    VERIFIED.get_or_init(Default::default)
}

/// The cache identity of one verification: the key, the message's BLAKE3 and
/// the signature, each length-prefixed so no two triples share an encoding.
pub(crate) fn verification_id(public_sec1: &[u8], message_digest: &[u8; 32], sig_der: &[u8]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(&(public_sec1.len() as u64).to_le_bytes());
    h.update(public_sec1);
    h.update(message_digest);
    h.update(&(sig_der.len() as u64).to_le_bytes());
    h.update(sig_der);
    *h.finalize().as_bytes()
}

pub(crate) fn already_verified(id: &[u8; 32]) -> bool {
    verified().lock().map(|s| s.contains(id)).unwrap_or(false)
}

pub(crate) fn remember_verified(id: [u8; 32]) {
    if let Ok(mut s) = verified().lock() {
        if s.len() >= VERIFIED_CAP {
            s.clear();
        }
        s.insert(id);
    }
}

/// Parse a SEC1 public key, `None` if it is not a valid P-256 point. A
/// compressed key costs a point decompression (a field square root), so a caller
/// verifying many signatures under one key parses it once.
pub(crate) fn parse_sec1(public_sec1: &[u8]) -> Option<VerifyingKey> {
    VerifyingKey::from_sec1_bytes(public_sec1).ok()
}

/// [`verify_sec1`] against an already-parsed key.
pub(crate) fn verify_with_key(key: &VerifyingKey, message: &[u8], sig_der: &[u8]) -> bool {
    match Signature::from_der(sig_der) {
        Ok(signature) => key.verify(message, &signature).is_ok(),
        Err(_) => false,
    }
}
