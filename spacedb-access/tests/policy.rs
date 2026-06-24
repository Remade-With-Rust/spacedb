//! M5-S3: the human-vs-AI access policy.

use spacedb_access::{
    gate, AccessPolicy, AccessRequest, Capability, CapabilityChain, Decision, DenyReason, Did,
    Identity, MemKeyDirectory, Ops, RevocationSet, Scope, SignedCapability,
};

const NOW: u64 = 1_000_000_000;
const FAR: u64 = 9_000_000_000;

fn published_owner() -> (Identity, MemKeyDirectory) {
    let owner = Identity::generate("did:mata:owner").unwrap();
    let dir = MemKeyDirectory::new();
    dir.publish(&owner).unwrap();
    (owner, dir)
}

#[test]
fn a_roster_human_reads_without_a_capability_but_not_writes() {
    let (_owner, dir) = published_owner();
    let human = Did::from("did:mata:alice");
    let policy = AccessPolicy::new().with_roster_member(human.clone());
    let scope = Scope::Collection("notes".into());

    let read = AccessRequest { bearer: &human, scope: &scope, op: Ops::READ };
    assert_eq!(
        gate(&policy, None, &read, &dir, NOW, &RevocationSet::new()).unwrap(),
        Decision::Allow
    );

    // a write is not a free read — it needs a capability
    let write = AccessRequest { bearer: &human, scope: &scope, op: Ops::WRITE };
    assert_eq!(
        gate(&policy, None, &write, &dir, NOW, &RevocationSet::new()).unwrap(),
        Decision::Deny(DenyReason::NoCapability)
    );
}

#[test]
fn an_agent_without_a_grant_is_hard_denied() {
    let (_owner, dir) = published_owner();
    let policy = AccessPolicy::new();
    let agent = Did::from("did:agent:bot");
    let scope = Scope::Collection("notes".into());
    let read = AccessRequest { bearer: &agent, scope: &scope, op: Ops::READ };
    assert_eq!(
        gate(&policy, None, &read, &dir, NOW, &RevocationSet::new()).unwrap(),
        Decision::Deny(DenyReason::NoCapability)
    );
}

#[test]
fn an_agent_with_a_grant_is_allowed() {
    let (owner, dir) = published_owner();
    let policy = AccessPolicy::new();
    let agent = Did::from("did:agent:bot");
    let scope = Scope::Collection("notes".into());

    let cap = Capability::grant(owner.did().clone(), agent.clone(), scope.clone(), Ops::READ)
        .unwrap()
        .with_expiry(FAR);
    let chain = CapabilityChain::single(SignedCapability::sign(cap, &owner).unwrap());

    let read = AccessRequest { bearer: &agent, scope: &scope, op: Ops::READ };
    assert_eq!(
        gate(&policy, Some(&chain), &read, &dir, NOW, &RevocationSet::new()).unwrap(),
        Decision::Allow
    );
}

#[test]
fn an_agents_grant_must_chain_to_an_accountable_root() {
    let owner = Identity::generate("did:mata:owner").unwrap();
    let rogue = Identity::generate("did:mata:rogue").unwrap();
    let dir = MemKeyDirectory::new();
    dir.publish(&owner).unwrap();
    dir.publish(&rogue).unwrap();

    let agent = Did::from("did:agent:bot");
    let scope = Scope::Collection("notes".into());
    let read = AccessRequest { bearer: &agent, scope: &scope, op: Ops::READ };

    // only `owner` is accountable; agents must chain to a roster member
    let policy = AccessPolicy::new()
        .with_roster_member(owner.did().clone())
        .requiring_accountable_agents();

    // granted by the accountable owner -> allowed
    let cap_ok = Capability::grant(owner.did().clone(), agent.clone(), scope.clone(), Ops::READ)
        .unwrap()
        .with_expiry(FAR);
    let chain_ok = CapabilityChain::single(SignedCapability::sign(cap_ok, &owner).unwrap());
    assert_eq!(
        gate(&policy, Some(&chain_ok), &read, &dir, NOW, &RevocationSet::new()).unwrap(),
        Decision::Allow
    );

    // granted by a rogue identity that's nobody's accountable root -> denied
    let cap_bad = Capability::grant(rogue.did().clone(), agent.clone(), scope.clone(), Ops::READ)
        .unwrap()
        .with_expiry(FAR);
    let chain_bad = CapabilityChain::single(SignedCapability::sign(cap_bad, &rogue).unwrap());
    assert_eq!(
        gate(&policy, Some(&chain_bad), &read, &dir, NOW, &RevocationSet::new()).unwrap(),
        Decision::Deny(DenyReason::NotAccountable)
    );
}
