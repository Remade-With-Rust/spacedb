//! M5-S1: minting and enforcing signed capabilities — the consent core.

use spacedb_access::{
    authorize, AccessRequest, Capability, Decision, DenyReason, Did, Identity, MemKeyDirectory,
    Ops, RevocationSet, Scope, SignedCapability,
};

const FAR_FUTURE: u64 = 9_000_000_000;
const NOW: u64 = 1_000_000_000;

fn no_revs() -> RevocationSet {
    RevocationSet::new()
}

/// An owner who has published their key, and an agent bearer.
fn owner_and_agent() -> (Identity, Did, MemKeyDirectory) {
    let owner = Identity::generate("did:mata:owner").unwrap();
    let dir = MemKeyDirectory::new();
    dir.publish(&owner).unwrap();
    (owner, Did::from("did:agent:bot"), dir)
}

fn read_request<'a>(bearer: &'a Did, scope: &'a Scope) -> AccessRequest<'a> {
    AccessRequest {
        bearer,
        scope,
        op: Ops::READ,
    }
}

#[test]
fn a_valid_grant_authorizes_the_request() {
    let (owner, agent, dir) = owner_and_agent();
    let cap = Capability::grant(owner.did().clone(), agent.clone(), Scope::Collection("notes".into()), Ops::READ)
        .unwrap()
        .with_expiry(FAR_FUTURE);
    let signed = SignedCapability::sign(cap, &owner).unwrap();

    // a collection grant covers a document within it
    let scope = Scope::Document {
        collection: "notes".into(),
        doc_id: "doc-1".into(),
    };
    let decision = authorize(&signed, &read_request(&agent, &scope), &dir, NOW, &no_revs()).unwrap();
    assert_eq!(decision, Decision::Allow);
    assert!(decision.is_allowed());
}

#[test]
fn a_different_bearer_is_denied() {
    let (owner, agent, dir) = owner_and_agent();
    let cap = Capability::grant(owner.did().clone(), agent, Scope::Collection("notes".into()), Ops::READ).unwrap();
    let signed = SignedCapability::sign(cap, &owner).unwrap();

    let impostor = Did::from("did:agent:other");
    let scope = Scope::Collection("notes".into());
    let decision = authorize(&signed, &read_request(&impostor, &scope), &dir, NOW, &no_revs()).unwrap();
    assert_eq!(decision, Decision::Deny(DenyReason::BearerMismatch));
}

#[test]
fn out_of_scope_is_denied() {
    let (owner, agent, dir) = owner_and_agent();
    let cap = Capability::grant(owner.did().clone(), agent.clone(), Scope::Collection("notes".into()), Ops::READ).unwrap();
    let signed = SignedCapability::sign(cap, &owner).unwrap();

    let other = Scope::Collection("secrets".into());
    let decision = authorize(&signed, &read_request(&agent, &other), &dir, NOW, &no_revs()).unwrap();
    assert_eq!(decision, Decision::Deny(DenyReason::OutOfScope));
}

#[test]
fn an_ungranted_op_is_denied() {
    let (owner, agent, dir) = owner_and_agent();
    // granted READ only
    let cap = Capability::grant(owner.did().clone(), agent.clone(), Scope::Collection("notes".into()), Ops::READ).unwrap();
    let signed = SignedCapability::sign(cap, &owner).unwrap();

    let scope = Scope::Collection("notes".into());
    let write = AccessRequest {
        bearer: &agent,
        scope: &scope,
        op: Ops::WRITE,
    };
    let decision = authorize(&signed, &write, &dir, NOW, &no_revs()).unwrap();
    assert_eq!(decision, Decision::Deny(DenyReason::OpNotGranted));
}

#[test]
fn an_expired_grant_is_denied() {
    let (owner, agent, dir) = owner_and_agent();
    let cap = Capability::grant(owner.did().clone(), agent.clone(), Scope::Collection("notes".into()), Ops::READ)
        .unwrap()
        .with_expiry(NOW); // expires exactly now
    let signed = SignedCapability::sign(cap, &owner).unwrap();

    let scope = Scope::Collection("notes".into());
    let decision = authorize(&signed, &read_request(&agent, &scope), &dir, NOW, &no_revs()).unwrap();
    assert_eq!(decision, Decision::Deny(DenyReason::Expired));
    // ...but valid just before expiry
    let before = authorize(&signed, &read_request(&agent, &scope), &dir, NOW - 1, &no_revs()).unwrap();
    assert_eq!(before, Decision::Allow);
}

#[test]
fn a_tampered_capability_fails_signature() {
    let (owner, agent, dir) = owner_and_agent();
    let cap = Capability::grant(owner.did().clone(), agent.clone(), Scope::Collection("notes".into()), Ops::READ).unwrap();
    let mut signed = SignedCapability::sign(cap, &owner).unwrap();

    // escalate the ops after signing — the signature no longer covers it
    signed.capability.ops = Ops::READ | Ops::WRITE | Ops::COMPUTE;
    let scope = Scope::Collection("notes".into());
    let decision = authorize(&signed, &read_request(&agent, &scope), &dir, NOW, &no_revs()).unwrap();
    assert_eq!(decision, Decision::Deny(DenyReason::BadSignature));
}

#[test]
fn an_unknown_issuer_is_denied() {
    let owner = Identity::generate("did:mata:owner").unwrap();
    let dir = MemKeyDirectory::new(); // owner NOT published
    let agent = Did::from("did:agent:bot");
    let cap = Capability::grant(owner.did().clone(), agent.clone(), Scope::Collection("notes".into()), Ops::READ).unwrap();
    let signed = SignedCapability::sign(cap, &owner).unwrap();

    let scope = Scope::Collection("notes".into());
    let decision = authorize(&signed, &read_request(&agent, &scope), &dir, NOW, &no_revs()).unwrap();
    assert_eq!(decision, Decision::Deny(DenyReason::UnknownIssuer));
}

#[test]
fn a_wrong_signer_is_denied() {
    // The capability claims `owner` as issuer, but is signed by someone else.
    let owner = Identity::generate("did:mata:owner").unwrap();
    let attacker = Identity::generate("did:mata:attacker").unwrap();
    let dir = MemKeyDirectory::new();
    dir.publish(&owner).unwrap();

    let agent = Did::from("did:agent:bot");
    let cap = Capability::grant(owner.did().clone(), agent.clone(), Scope::Collection("notes".into()), Ops::READ).unwrap();
    let forged = SignedCapability::sign(cap, &attacker).unwrap(); // signed by the wrong key

    let scope = Scope::Collection("notes".into());
    let decision = authorize(&forged, &read_request(&agent, &scope), &dir, NOW, &no_revs()).unwrap();
    assert_eq!(decision, Decision::Deny(DenyReason::BadSignature));
}

#[test]
fn document_scope_does_not_cover_other_documents_or_the_collection() {
    let (owner, agent, dir) = owner_and_agent();
    let cap = Capability::grant(
        owner.did().clone(),
        agent.clone(),
        Scope::Document {
            collection: "notes".into(),
            doc_id: "doc-1".into(),
        },
        Ops::READ,
    )
    .unwrap();
    let signed = SignedCapability::sign(cap, &owner).unwrap();

    // exact document: allowed
    let exact = Scope::Document {
        collection: "notes".into(),
        doc_id: "doc-1".into(),
    };
    assert_eq!(authorize(&signed, &read_request(&agent, &exact), &dir, NOW, &no_revs()).unwrap(), Decision::Allow);

    // a different document: denied
    let other_doc = Scope::Document {
        collection: "notes".into(),
        doc_id: "doc-2".into(),
    };
    assert_eq!(
        authorize(&signed, &read_request(&agent, &other_doc), &dir, NOW, &no_revs()).unwrap(),
        Decision::Deny(DenyReason::OutOfScope)
    );

    // the whole collection: denied
    let whole = Scope::Collection("notes".into());
    assert_eq!(
        authorize(&signed, &read_request(&agent, &whole), &dir, NOW, &no_revs()).unwrap(),
        Decision::Deny(DenyReason::OutOfScope)
    );
}

#[test]
fn function_compute_scope_authorizes_the_function() {
    let (owner, agent, dir) = owner_and_agent();
    let cap = Capability::grant(
        owner.did().clone(),
        agent.clone(),
        Scope::Function("rank".into()),
        Ops::COMPUTE,
    )
    .unwrap();
    let signed = SignedCapability::sign(cap, &owner).unwrap();

    let scope = Scope::Function("rank".into());
    let req = AccessRequest {
        bearer: &agent,
        scope: &scope,
        op: Ops::COMPUTE,
    };
    assert_eq!(authorize(&signed, &req, &dir, NOW, &no_revs()).unwrap(), Decision::Allow);
}

#[test]
fn signed_capability_round_trips_through_serde() {
    let (owner, agent, _dir) = owner_and_agent();
    let cap = Capability::grant(owner.did().clone(), agent, Scope::Collection("notes".into()), Ops::READ | Ops::WRITE)
        .unwrap()
        .with_budget(5_000)
        .with_delegation_depth(2);
    let signed = SignedCapability::sign(cap, &owner).unwrap();

    let bytes = signed.encode().unwrap();
    let decoded = SignedCapability::decode(&bytes).unwrap();
    assert_eq!(decoded, signed);
}
