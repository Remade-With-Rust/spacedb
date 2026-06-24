//! Human-vs-AI access policy — the AI-age rule.
//!
//! Capabilities say *what a bearer may do*; policy says *who needs one*. The
//! default posture: **humans in the owner's roster read freely; everyone else —
//! every AI agent — needs an explicit, signed grant**, and (optionally) an
//! agent's grant chain must root at an accountable roster member, so the agent's
//! authority always traces back to a human/org.
//!
//! [`gate`] combines the policy with capability authorization into one decision.
//!
//! ## The honest boundary
//!
//! A capability (and this gate) grants **permission + accountability + billing**,
//! **not confidentiality on untrusted compute**. The Phase-1 rule is: access is
//! allowed only when it is *authorized-by-mID* **and** *executed on a node
//! entitled to decrypt* (the owner's roster, holding the per-collection DEK).
//! The second half is enforced by the storage layer's `KeyProvider` — a node
//! without the key simply cannot read the ciphertext — not re-implemented here.
//! Untrusted-node confidential compute (enclaves / FHE) is Phase 2.

use std::collections::HashSet;

use crate::authorize::{authorize_chain, AccessRequest, Decision, DenyReason};
use crate::capability::Ops;
use crate::chain::CapabilityChain;
use crate::directory::KeyDirectory;
use crate::error::AccessResult;
use crate::identity::Did;
use crate::revocation::RevocationSet;

/// Who reads freely, and whether agents must chain to an accountable identity.
#[derive(Clone, Debug, Default)]
pub struct AccessPolicy {
    roster: HashSet<Did>,
    require_accountable_root: bool,
}

impl AccessPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an accountable human/org to the roster (builder style).
    pub fn with_roster_member(mut self, did: impl Into<Did>) -> Self {
        self.roster.insert(did.into());
        self
    }

    /// Require that an agent's grant chain root at a roster member.
    pub fn requiring_accountable_agents(mut self) -> Self {
        self.require_accountable_root = true;
        self
    }

    /// Whether `did` is an accountable roster member.
    pub fn is_roster(&self, did: &Did) -> bool {
        self.roster.contains(did)
    }
}

/// Decide an access under policy + capabilities.
///
/// - A roster member performing a pure **read** is allowed with no capability.
/// - Otherwise a capability chain is **required**; it is authorized normally
///   (signature · narrowing · revocation · scope · ops · expiry).
/// - If `require_accountable_root`, the chain must root at a roster member.
pub fn gate(
    policy: &AccessPolicy,
    chain: Option<&CapabilityChain>,
    request: &AccessRequest,
    directory: &dyn KeyDirectory,
    now_unix: u64,
    revocations: &RevocationSet,
) -> AccessResult<Decision> {
    // Roster humans read freely (no grant needed).
    if request.op == Ops::READ && policy.is_roster(request.bearer) {
        return Ok(Decision::Allow);
    }

    // Everyone else needs a valid capability.
    let chain = match chain {
        Some(c) => c,
        None => return Ok(Decision::Deny(DenyReason::NoCapability)),
    };

    let decision = authorize_chain(chain, request, directory, now_unix, revocations)?;
    if !decision.is_allowed() {
        return Ok(decision);
    }

    // The grant must trace back to an accountable identity.
    if policy.require_accountable_root {
        let root_issuer = &chain.links()[0].capability.issuer;
        if !policy.is_roster(root_issuer) {
            return Ok(Decision::Deny(DenyReason::NotAccountable));
        }
    }

    Ok(Decision::Allow)
}
