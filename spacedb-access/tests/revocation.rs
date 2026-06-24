//! M5-S2: revocation — immediate and fail-closed.

use spacedb_access::{
    authorize, AccessRequest, Capability, Decision, DenyReason, Did, Identity, MemKeyDirectory,
    Ops, RevocationSet, Scope, SignedCapability,
};

const NOW: u64 = 1_000_000_000;

fn published_owner() -> (Identity, MemKeyDirectory) {
    let owner = Identity::generate("did:mata:owner").unwrap();
    let dir = MemKeyDirectory::new();
    dir.publish(&owner).unwrap();
    (owner, dir)
}

#[test]
fn revoking_a_capability_denies_it_immediately() {
    let (owner, dir) = published_owner();
    let agent = Did::from("did:agent:bot");
    let cap = Capability::grant(owner.did().clone(), agent.clone(), Scope::Collection("notes".into()), Ops::READ).unwrap();
    let cap_id = cap.id;
    let signed = SignedCapability::sign(cap, &owner).unwrap();

    let scope = Scope::Collection("notes".into());
    let req = AccessRequest {
        bearer: &agent,
        scope: &scope,
        op: Ops::READ,
    };

    let mut revocations = RevocationSet::new();
    assert_eq!(authorize(&signed, &req, &dir, NOW, &revocations).unwrap(), Decision::Allow);

    revocations.revoke(cap_id);
    assert_eq!(
        authorize(&signed, &req, &dir, NOW, &revocations).unwrap(),
        Decision::Deny(DenyReason::Revoked)
    );
}

#[test]
fn revocation_only_affects_the_named_capability() {
    let (owner, dir) = published_owner();
    let agent = Did::from("did:agent:bot");

    let cap_a = Capability::grant(owner.did().clone(), agent.clone(), Scope::Collection("a".into()), Ops::READ).unwrap();
    let id_a = cap_a.id;
    let signed_a = SignedCapability::sign(cap_a, &owner).unwrap();
    let cap_b = Capability::grant(owner.did().clone(), agent.clone(), Scope::Collection("b".into()), Ops::READ).unwrap();
    let signed_b = SignedCapability::sign(cap_b, &owner).unwrap();

    let mut revocations = RevocationSet::new();
    revocations.revoke(id_a);

    let scope_a = Scope::Collection("a".into());
    let scope_b = Scope::Collection("b".into());
    let req_a = AccessRequest { bearer: &agent, scope: &scope_a, op: Ops::READ };
    let req_b = AccessRequest { bearer: &agent, scope: &scope_b, op: Ops::READ };

    assert_eq!(authorize(&signed_a, &req_a, &dir, NOW, &revocations).unwrap(), Decision::Deny(DenyReason::Revoked));
    assert_eq!(authorize(&signed_b, &req_b, &dir, NOW, &revocations).unwrap(), Decision::Allow);
}
