//! The enforcement engine: does this credential authorize this access, right now?
//!
//! [`authorize`] handles a single (root) capability; [`authorize_chain`] handles a
//! delegation chain. Both check, for every link: the signature (against the
//! issuer's published key) and revocation; the chain additionally enforces that
//! each sub-grant **narrows** its parent (issuer = delegator, scope ⊆, ops ⊆,
//! expiry ≤, depth −1). The leaf is then checked against the request (bearer,
//! scope, ops, expiry). Every failure is a typed [`DenyReason`] — denial is a
//! normal result, not an error. The caller supplies `now_unix`, so the engine
//! stays deterministic.

use serde::{Deserialize, Serialize};

use crate::capability::{Capability, Ops, Scope, SignedCapability};
use crate::chain::CapabilityChain;
use crate::directory::KeyDirectory;
use crate::error::AccessResult;
use crate::identity::{verify_sec1, Did};
use crate::revocation::RevocationSet;

/// A request to access something, presented by a bearer.
pub struct AccessRequest<'a> {
    pub bearer: &'a Did,
    pub scope: &'a Scope,
    pub op: Ops,
}

/// Why a delegation link was invalid.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DelegationError {
    /// A sub-grant's issuer is not the bearer of the link above it.
    IssuerNotDelegator,
    /// The parent grant is not delegable (`delegation_depth == 0`).
    ParentNotDelegable,
    /// The sub-grant's delegation depth exceeds `parent.depth - 1`.
    DepthExceeded,
    /// The sub-grant's scope is broader than its parent's.
    ScopeEscalation,
    /// The sub-grant requests operations its parent did not have.
    OpsEscalation,
    /// The sub-grant would outlive its parent.
    ExpiryExtension,
}

/// Why an access was denied.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DenyReason {
    UnknownIssuer,
    BadSignature,
    BearerMismatch,
    OutOfScope,
    OpNotGranted,
    Expired,
    /// The capability (or an ancestor in its chain) has been revoked.
    Revoked,
    /// A delegation link is invalid.
    Delegation(DelegationError),
    /// An empty capability chain was presented.
    EmptyChain,
    /// Policy required a capability, but none was presented (e.g. an AI agent
    /// with no grant).
    NoCapability,
    /// Policy required the grant chain to root at an accountable roster member,
    /// and it did not.
    NotAccountable,
}

/// The authorization decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny(DenyReason),
}

impl Decision {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Decision::Allow)
    }
}

enum LinkSig {
    Ok,
    UnknownIssuer,
    BadSignature,
}

fn verify_link_signature(
    link: &SignedCapability,
    directory: &dyn KeyDirectory,
) -> AccessResult<LinkSig> {
    let key = match directory.published_key(&link.capability.issuer)? {
        Some(k) => k,
        None => return Ok(LinkSig::UnknownIssuer),
    };
    let canonical = link.capability.canonical_bytes()?;
    if verify_sec1(&key, &canonical, &link.issuer_signature) {
        Ok(LinkSig::Ok)
    } else {
        Ok(LinkSig::BadSignature)
    }
}

/// Check the leaf capability against the actual request.
fn check_request(cap: &Capability, request: &AccessRequest, now_unix: u64) -> Option<DenyReason> {
    if &cap.bearer != request.bearer {
        return Some(DenyReason::BearerMismatch);
    }
    if !cap.scope.covers(request.scope) {
        return Some(DenyReason::OutOfScope);
    }
    if !cap.ops.contains(request.op) {
        return Some(DenyReason::OpNotGranted);
    }
    if let Some(expiry) = cap.expiry {
        if now_unix >= expiry {
            return Some(DenyReason::Expired);
        }
    }
    None
}

fn expiry_within(parent: Option<u64>, sub: Option<u64>) -> bool {
    match (parent, sub) {
        (None, _) => true,             // parent never expires; sub may be anything
        (Some(_), None) => false,      // parent expires; a forever sub would outlive it
        (Some(p), Some(s)) => s <= p,  // sub must not outlive parent
    }
}

/// Check a sub-grant narrows its parent.
fn check_narrowing(parent: &Capability, sub: &Capability) -> Option<DelegationError> {
    if sub.issuer != parent.bearer {
        return Some(DelegationError::IssuerNotDelegator);
    }
    if parent.delegation_depth == 0 {
        return Some(DelegationError::ParentNotDelegable);
    }
    if sub.delegation_depth > parent.delegation_depth - 1 {
        return Some(DelegationError::DepthExceeded);
    }
    if !parent.scope.covers(&sub.scope) {
        return Some(DelegationError::ScopeEscalation);
    }
    if !sub.ops.is_subset_of(parent.ops) {
        return Some(DelegationError::OpsEscalation);
    }
    if !expiry_within(parent.expiry, sub.expiry) {
        return Some(DelegationError::ExpiryExtension);
    }
    None
}

/// Authorize `request` against a single (root) capability.
pub fn authorize(
    signed: &SignedCapability,
    request: &AccessRequest,
    directory: &dyn KeyDirectory,
    now_unix: u64,
    revocations: &RevocationSet,
) -> AccessResult<Decision> {
    match verify_link_signature(signed, directory)? {
        LinkSig::UnknownIssuer => return Ok(Decision::Deny(DenyReason::UnknownIssuer)),
        LinkSig::BadSignature => return Ok(Decision::Deny(DenyReason::BadSignature)),
        LinkSig::Ok => {}
    }
    if revocations.is_revoked(&signed.capability.id) {
        return Ok(Decision::Deny(DenyReason::Revoked));
    }
    if let Some(reason) = check_request(&signed.capability, request, now_unix) {
        return Ok(Decision::Deny(reason));
    }
    Ok(Decision::Allow)
}

/// Authorize `request` against a delegation chain: verify every link's signature,
/// that each link narrows its parent, that no link is revoked, and that the leaf
/// satisfies the request.
pub fn authorize_chain(
    chain: &CapabilityChain,
    request: &AccessRequest,
    directory: &dyn KeyDirectory,
    now_unix: u64,
    revocations: &RevocationSet,
) -> AccessResult<Decision> {
    let links = chain.links();
    if links.is_empty() {
        return Ok(Decision::Deny(DenyReason::EmptyChain));
    }

    let mut parent: Option<&Capability> = None;
    for link in links {
        match verify_link_signature(link, directory)? {
            LinkSig::UnknownIssuer => return Ok(Decision::Deny(DenyReason::UnknownIssuer)),
            LinkSig::BadSignature => return Ok(Decision::Deny(DenyReason::BadSignature)),
            LinkSig::Ok => {}
        }
        if revocations.is_revoked(&link.capability.id) {
            return Ok(Decision::Deny(DenyReason::Revoked));
        }
        if let Some(p) = parent {
            if let Some(err) = check_narrowing(p, &link.capability) {
                return Ok(Decision::Deny(DenyReason::Delegation(err)));
            }
        }
        parent = Some(&link.capability);
    }

    // The leaf is the credential actually being exercised.
    let leaf = &links.last().unwrap().capability;
    if let Some(reason) = check_request(leaf, request, now_unix) {
        return Ok(Decision::Deny(reason));
    }
    Ok(Decision::Allow)
}
