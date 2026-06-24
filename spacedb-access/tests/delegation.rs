//! M5-S2: bounded delegation — sub-grants that may only narrow, chain-verified.

use spacedb_access::{
    authorize_chain, delegate, AccessRequest, Capability, CapabilityChain, Decision,
    DelegationError, DenyReason, Did, Identity, MemKeyDirectory, Ops, RevocationSet, Scope,
    SignedCapability,
};

const NOW: u64 = 1_000_000_000;
const FAR: u64 = 9_000_000_000;

fn coll(name: &str) -> Scope {
    Scope::Collection(name.into())
}
fn doc(c: &str, d: &str) -> Scope {
    Scope::Document {
        collection: c.into(),
        doc_id: d.into(),
    }
}

/// A signed root grant from `owner` to `bearer`, returning the chain and the root
/// capability's id (for revocation tests).
fn signed_root(
    owner: &Identity,
    bearer: &Did,
    scope: Scope,
    ops: Ops,
    depth: u8,
    expiry: u64,
) -> (CapabilityChain, [u8; 16]) {
    let cap = Capability::grant(owner.did().clone(), bearer.clone(), scope, ops)
        .unwrap()
        .with_delegation_depth(depth)
        .with_expiry(expiry);
    let id = cap.id;
    (
        CapabilityChain::single(SignedCapability::sign(cap, owner).unwrap()),
        id,
    )
}

fn sub(issuer: &Did, bearer: &Did, scope: Scope, ops: Ops, depth: u8, expiry: u64) -> Capability {
    Capability::grant(issuer.clone(), bearer.clone(), scope, ops)
        .unwrap()
        .with_delegation_depth(depth)
        .with_expiry(expiry)
}

fn assert_delegation_denied(
    chain: &CapabilityChain,
    bearer: &Did,
    scope: &Scope,
    dir: &MemKeyDirectory,
    expected: DelegationError,
) {
    let req = AccessRequest {
        bearer,
        scope,
        op: Ops::READ,
    };
    assert_eq!(
        authorize_chain(chain, &req, dir, NOW, &RevocationSet::new()).unwrap(),
        Decision::Deny(DenyReason::Delegation(expected))
    );
}

/// owner + delegating agent a1 (both published) + final bearer a2.
fn fixture() -> (Identity, Identity, Did, MemKeyDirectory) {
    let owner = Identity::generate("did:mata:owner").unwrap();
    let a1 = Identity::generate("did:agent:one").unwrap();
    let dir = MemKeyDirectory::new();
    dir.publish(&owner).unwrap();
    dir.publish(&a1).unwrap();
    (owner, a1, Did::from("did:agent:two"), dir)
}

#[test]
fn a_valid_subgrant_authorizes() {
    let (owner, a1, a2, dir) = fixture();
    let (root, _) = signed_root(&owner, a1.did(), coll("notes"), Ops::READ | Ops::WRITE, 1, FAR);
    let chain = delegate(&root, sub(a1.did(), &a2, doc("notes", "d1"), Ops::READ, 0, FAR), &a1).unwrap();

    let scope = doc("notes", "d1");
    let req = AccessRequest {
        bearer: &a2,
        scope: &scope,
        op: Ops::READ,
    };
    assert_eq!(
        authorize_chain(&chain, &req, &dir, NOW, &RevocationSet::new()).unwrap(),
        Decision::Allow
    );
}

#[test]
fn scope_escalation_is_denied() {
    let (owner, a1, a2, dir) = fixture();
    // parent is a single document; sub tries to widen to the whole collection
    let (root, _) = signed_root(&owner, a1.did(), doc("notes", "d1"), Ops::READ, 1, FAR);
    let chain = delegate(&root, sub(a1.did(), &a2, coll("notes"), Ops::READ, 0, FAR), &a1).unwrap();
    assert_delegation_denied(&chain, &a2, &coll("notes"), &dir, DelegationError::ScopeEscalation);
}

#[test]
fn ops_escalation_is_denied() {
    let (owner, a1, a2, dir) = fixture();
    let (root, _) = signed_root(&owner, a1.did(), coll("notes"), Ops::READ, 1, FAR);
    let chain = delegate(&root, sub(a1.did(), &a2, coll("notes"), Ops::READ | Ops::WRITE, 0, FAR), &a1).unwrap();
    assert_delegation_denied(&chain, &a2, &coll("notes"), &dir, DelegationError::OpsEscalation);
}

#[test]
fn expiry_extension_is_denied() {
    let (owner, a1, a2, dir) = fixture();
    let (root, _) = signed_root(&owner, a1.did(), coll("notes"), Ops::READ, 1, FAR);
    // sub would outlive the parent
    let chain = delegate(&root, sub(a1.did(), &a2, coll("notes"), Ops::READ, 0, FAR + 1), &a1).unwrap();
    assert_delegation_denied(&chain, &a2, &coll("notes"), &dir, DelegationError::ExpiryExtension);
}

#[test]
fn depth_exceeded_is_denied() {
    let (owner, a1, a2, dir) = fixture();
    // parent depth 1 allows sub depth <= 0; sub asks for 1
    let (root, _) = signed_root(&owner, a1.did(), coll("notes"), Ops::READ, 1, FAR);
    let chain = delegate(&root, sub(a1.did(), &a2, coll("notes"), Ops::READ, 1, FAR), &a1).unwrap();
    assert_delegation_denied(&chain, &a2, &coll("notes"), &dir, DelegationError::DepthExceeded);
}

#[test]
fn a_non_delegable_parent_cannot_delegate() {
    let (owner, a1, a2, dir) = fixture();
    let (root, _) = signed_root(&owner, a1.did(), coll("notes"), Ops::READ, 0, FAR); // depth 0
    let chain = delegate(&root, sub(a1.did(), &a2, coll("notes"), Ops::READ, 0, FAR), &a1).unwrap();
    assert_delegation_denied(&chain, &a2, &coll("notes"), &dir, DelegationError::ParentNotDelegable);
}

#[test]
fn a_subgrant_not_issued_by_the_delegator_is_denied() {
    let (owner, a1, a2, dir) = fixture();
    // a3 is a real, published identity, but it is NOT the parent's bearer (a1)
    let a3 = Identity::generate("did:agent:three").unwrap();
    dir.publish(&a3).unwrap();
    let (root, _) = signed_root(&owner, a1.did(), coll("notes"), Ops::READ, 1, FAR);
    // signed validly by a3, claiming a3 as issuer — but a3 never held the parent
    let chain = delegate(&root, sub(a3.did(), &a2, coll("notes"), Ops::READ, 0, FAR), &a3).unwrap();
    assert_delegation_denied(&chain, &a2, &coll("notes"), &dir, DelegationError::IssuerNotDelegator);
}

#[test]
fn a_two_level_chain_authorizes_the_leaf() {
    let owner = Identity::generate("did:mata:owner").unwrap();
    let a1 = Identity::generate("did:agent:one").unwrap();
    let a2 = Identity::generate("did:agent:two").unwrap();
    let a3 = Did::from("did:agent:three");
    let dir = MemKeyDirectory::new();
    dir.publish(&owner).unwrap();
    dir.publish(&a1).unwrap();
    dir.publish(&a2).unwrap();

    let (root, _) = signed_root(&owner, a1.did(), coll("notes"), Ops::READ, 2, FAR);
    let chain1 = delegate(&root, sub(a1.did(), a2.did(), coll("notes"), Ops::READ, 1, FAR), &a1).unwrap();
    let chain2 = delegate(&chain1, sub(a2.did(), &a3, doc("notes", "d1"), Ops::READ, 0, FAR), &a2).unwrap();
    assert_eq!(chain2.len(), 3);

    let scope = doc("notes", "d1");
    let req = AccessRequest {
        bearer: &a3,
        scope: &scope,
        op: Ops::READ,
    };
    assert_eq!(
        authorize_chain(&chain2, &req, &dir, NOW, &RevocationSet::new()).unwrap(),
        Decision::Allow
    );
}

#[test]
fn revoking_the_root_kills_the_whole_chain() {
    let (owner, a1, a2, dir) = fixture();
    let (root, root_id) = signed_root(&owner, a1.did(), coll("notes"), Ops::READ, 1, FAR);
    let chain = delegate(&root, sub(a1.did(), &a2, doc("notes", "d1"), Ops::READ, 0, FAR), &a1).unwrap();

    let mut revocations = RevocationSet::new();
    revocations.revoke(root_id);

    let scope = doc("notes", "d1");
    let req = AccessRequest {
        bearer: &a2,
        scope: &scope,
        op: Ops::READ,
    };
    assert_eq!(
        authorize_chain(&chain, &req, &dir, NOW, &revocations).unwrap(),
        Decision::Deny(DenyReason::Revoked)
    );
}

#[test]
fn an_unpublished_delegator_key_is_denied() {
    let owner = Identity::generate("did:mata:owner").unwrap();
    let a1 = Identity::generate("did:agent:one").unwrap();
    let a2 = Did::from("did:agent:two");
    let dir = MemKeyDirectory::new();
    dir.publish(&owner).unwrap(); // a1 deliberately NOT published

    let (root, _) = signed_root(&owner, a1.did(), coll("notes"), Ops::READ, 1, FAR);
    let chain = delegate(&root, sub(a1.did(), &a2, coll("notes"), Ops::READ, 0, FAR), &a1).unwrap();

    let scope = coll("notes");
    let req = AccessRequest {
        bearer: &a2,
        scope: &scope,
        op: Ops::READ,
    };
    assert_eq!(
        authorize_chain(&chain, &req, &dir, NOW, &RevocationSet::new()).unwrap(),
        Decision::Deny(DenyReason::UnknownIssuer)
    );
}

#[test]
fn capability_chain_round_trips_through_its_codec() {
    let (owner, a1, a2, _dir) = fixture();
    let (root, _) = signed_root(&owner, a1.did(), coll("notes"), Ops::READ, 1, FAR);
    let chain = delegate(&root, sub(a1.did(), &a2, doc("notes", "d1"), Ops::READ, 0, FAR), &a1).unwrap();
    let bytes = chain.encode().unwrap();
    assert_eq!(CapabilityChain::decode(&bytes).unwrap(), chain);
}
