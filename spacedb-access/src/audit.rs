//! The access audit log — signed, content-addressed, append-only.
//!
//! Every access (allowed *or* denied) becomes an [`AuditEntry`] in the serving
//! node's [`AuditLog`]: hash-chained (each entry commits to the previous, so the
//! log is append-only and tamper-evident, like the Iron Bank journal) and signed
//! by the node (so the owner can attribute the log to their node and detect
//! forgery). The owner reads this log to see who/what/when, and revokes from it.
//!
//! [`AuditLog::verify`] re-walks the chain: sequence, prev-hash links, each
//! entry's content hash, and each node signature.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::authorize::{Decision, DenyReason};
use crate::capability::{Ops, Scope};
use p256::ecdsa::VerifyingKey;

use crate::identity::{
    already_verified, parse_sec1, remember_verified, verification_id, verify_with_key, Did,
    Identity,
};

pub type AuditResult<T> = Result<T, AuditError>;

#[derive(Debug, Error)]
pub enum AuditError {
    #[error("audit canonicalization: {0}")]
    Canonical(String),
    #[error("audit chain broken at seq {0}")]
    BrokenChain(u64),
    #[error("audit entry hash mismatch at seq {0}")]
    HashMismatch(u64),
    #[error("audit node signature invalid at seq {0}")]
    BadSignature(u64),
    #[error("audit sequence mismatch at index {0}")]
    SeqMismatch(u64),
}

/// What was decided, recorded for the audit trail.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuditDecision {
    Allowed,
    Denied(DenyReason),
}

impl AuditDecision {
    /// Map an authorization [`Decision`] into its audit form.
    pub fn of(decision: &Decision) -> Self {
        match decision {
            Decision::Allow => AuditDecision::Allowed,
            Decision::Deny(reason) => AuditDecision::Denied(reason.clone()),
        }
    }
}

/// One immutable, signed entry in the access log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub seq: u64,
    /// Hash of the previous entry (all-zero for the first) — the chain link.
    pub prev_hash: [u8; 32],
    pub at_unix: u64,
    pub bearer: Did,
    pub scope: Scope,
    pub op: Ops,
    pub capability_id: Option<[u8; 16]>,
    pub decision: AuditDecision,
    /// BLAKE3 of this entry's canonical content (including `prev_hash`).
    pub entry_hash: [u8; 32],
    /// The node's signature over the canonical content.
    pub node_signature: Vec<u8>,
}

/// The canonical content of an entry, encoded into `out` (cleared first) so a
/// walk over many entries reuses one buffer.
#[allow(clippy::too_many_arguments)]
fn content_bytes_into(
    out: &mut Vec<u8>,
    seq: u64,
    prev_hash: &[u8; 32],
    at_unix: u64,
    bearer: &Did,
    scope: &Scope,
    op: Ops,
    capability_id: &Option<[u8; 16]>,
    decision: &AuditDecision,
) -> AuditResult<()> {
    out.clear();
    let buf = std::mem::take(out);
    *out = postcard::serialize_with_flavor(
        &(seq, prev_hash, at_unix, bearer, scope, op, capability_id, decision),
        AppendVec(buf),
    )
    .map_err(|e| AuditError::Canonical(e.to_string()))?;
    Ok(())
}

/// A postcard output appending to an existing `Vec` with whole-slice copies.
struct AppendVec(Vec<u8>);

impl postcard::ser_flavors::Flavor for AppendVec {
    type Output = Vec<u8>;

    fn try_push(&mut self, byte: u8) -> postcard::Result<()> {
        self.0.push(byte);
        Ok(())
    }

    fn try_extend(&mut self, bytes: &[u8]) -> postcard::Result<()> {
        self.0.extend_from_slice(bytes);
        Ok(())
    }

    fn finalize(self) -> postcard::Result<Vec<u8>> {
        Ok(self.0)
    }
}

fn hash(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}

/// A node's append-only access log.
#[derive(Clone, Debug, Default)]
pub struct AuditLog {
    entries: Vec<AuditEntry>,
    /// Reused encoding buffer for `record` (not part of the log).
    scratch: Vec<u8>,
}

impl AuditLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reconstruct a log from persisted entries (then [`verify`](Self::verify)).
    pub fn from_entries(entries: Vec<AuditEntry>) -> Self {
        Self {
            entries,
            scratch: Vec::new(),
        }
    }

    pub fn entries(&self) -> &[AuditEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Append a signed, chained record of one access decision.
    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &mut self,
        node: &Identity,
        at_unix: u64,
        bearer: &Did,
        scope: &Scope,
        op: Ops,
        capability_id: Option<[u8; 16]>,
        decision: AuditDecision,
    ) -> AuditResult<()> {
        let seq = self.entries.len() as u64;
        let prev_hash = self
            .entries
            .last()
            .map(|e| e.entry_hash)
            .unwrap_or([0u8; 32]);

        content_bytes_into(
            &mut self.scratch,
            seq,
            &prev_hash,
            at_unix,
            bearer,
            scope,
            op,
            &capability_id,
            &decision,
        )?;
        let entry_hash = hash(&self.scratch);
        let node_signature = node.sign(&self.scratch);
        remember_verified(verification_id(node.public_key(), &entry_hash, &node_signature));

        self.entries.push(AuditEntry {
            seq,
            prev_hash,
            at_unix,
            bearer: bearer.clone(),
            scope: scope.clone(),
            op,
            capability_id,
            decision,
            entry_hash,
            node_signature,
        });
        Ok(())
    }

    /// Re-walk the chain: sequence, prev-hash links, content hashes, and each
    /// node signature against `node_public_key`. Any inconsistency is an error.
    pub fn verify(&self, node_public_key: &[u8]) -> AuditResult<()> {
        // One key for the whole log, parsed at most once, and only if some
        // entry is not already known-verified. A key that does not parse fails
        // the first entry that reaches the signature check, as before.
        let mut node_key: Option<Option<VerifyingKey>> = None;
        let mut canonical = Vec::new();
        let mut expected_prev = [0u8; 32];
        for (index, entry) in self.entries.iter().enumerate() {
            if entry.seq != index as u64 {
                return Err(AuditError::SeqMismatch(index as u64));
            }
            if entry.prev_hash != expected_prev {
                return Err(AuditError::BrokenChain(entry.seq));
            }
            content_bytes_into(
                &mut canonical,
                entry.seq,
                &entry.prev_hash,
                entry.at_unix,
                &entry.bearer,
                &entry.scope,
                entry.op,
                &entry.capability_id,
                &entry.decision,
            )?;
            if hash(&canonical) != entry.entry_hash {
                return Err(AuditError::HashMismatch(entry.seq));
            }
            // `entry_hash` has just been checked to be BLAKE3(canonical): it is
            // the message digest the verified-signature cache is keyed by.
            let id = verification_id(node_public_key, &entry.entry_hash, &entry.node_signature);
            let signed = already_verified(&id) || {
                let ok = node_key
                    .get_or_insert_with(|| parse_sec1(node_public_key))
                    .as_ref()
                    .is_some_and(|key| verify_with_key(key, &canonical, &entry.node_signature));
                if ok {
                    remember_verified(id);
                }
                ok
            };
            if !signed {
                return Err(AuditError::BadSignature(entry.seq));
            }
            expected_prev = entry.entry_hash;
        }
        Ok(())
    }
}
