//! M5 ship criterion: an AI agent with its own mID is granted scoped read+compute
//! by the owner, uses it, the owner revokes it, an un-granted agent is hard-denied
//! — and every access (allowed and denied) is in the owner's signed audit log.

use spacedb_access::{
    gate, AccessPolicy, AccessRequest, AuditDecision, Capability, CapabilityChain, Decision,
    DenyReason, Did, Identity, MemKeyDirectory, Ops, RevocationSet, Scope, SignedCapability,
};

const NOW: u64 = 1_000_000_000;
const FAR: u64 = 9_000_000_000;

#[test]
fn end_to_end_consent_flow_is_fully_audited() {
    // owner (accountable), the serving node, and an agent with its own identity.
    let owner = Identity::generate("did:mata:owner").unwrap();
    let node = Identity::generate("did:mata:node").unwrap();
    let dir = MemKeyDirectory::new();
    dir.publish(&owner).unwrap();
    let agent = Did::from("did:agent:assistant");
    let scope = Scope::Collection("journal".into());

    let policy = AccessPolicy::new()
        .with_roster_member(owner.did().clone())
        .requiring_accountable_agents();
    let mut log = spacedb_access::AuditLog::new();
    let mut revocations = RevocationSet::new();

    // The owner grants the agent scoped read + compute, budgeted and expiring.
    let cap = Capability::grant(owner.did().clone(), agent.clone(), scope.clone(), Ops::READ | Ops::COMPUTE)
        .unwrap()
        .with_expiry(FAR)
        .with_budget(10_000);
    let cap_id = cap.id;
    let chain = CapabilityChain::single(SignedCapability::sign(cap, &owner).unwrap());

    // 1. the agent reads — allowed, audited
    let read = AccessRequest { bearer: &agent, scope: &scope, op: Ops::READ };
    let d1 = gate(&policy, Some(&chain), &read, &dir, NOW, &revocations).unwrap();
    assert_eq!(d1, Decision::Allow);
    log.record(&node, NOW, &agent, &scope, Ops::READ, Some(cap_id), AuditDecision::of(&d1)).unwrap();

    // 2. the agent runs an on-node compute — allowed, audited
    let compute = AccessRequest { bearer: &agent, scope: &scope, op: Ops::COMPUTE };
    let d2 = gate(&policy, Some(&chain), &compute, &dir, NOW + 1, &revocations).unwrap();
    assert_eq!(d2, Decision::Allow);
    log.record(&node, NOW + 1, &agent, &scope, Ops::COMPUTE, Some(cap_id), AuditDecision::of(&d2)).unwrap();

    // 3. the owner revokes — the agent's next access is denied, audited
    revocations.revoke(cap_id);
    let d3 = gate(&policy, Some(&chain), &read, &dir, NOW + 2, &revocations).unwrap();
    assert_eq!(d3, Decision::Deny(DenyReason::Revoked));
    log.record(&node, NOW + 2, &agent, &scope, Ops::READ, Some(cap_id), AuditDecision::of(&d3)).unwrap();

    // 4. an un-granted agent is hard-denied, audited
    let stranger = Did::from("did:agent:stranger");
    let read2 = AccessRequest { bearer: &stranger, scope: &scope, op: Ops::READ };
    let d4 = gate(&policy, None, &read2, &dir, NOW + 3, &revocations).unwrap();
    assert_eq!(d4, Decision::Deny(DenyReason::NoCapability));
    log.record(&node, NOW + 3, &stranger, &scope, Ops::READ, None, AuditDecision::of(&d4)).unwrap();

    // The owner's log is intact, attributable, and tells the whole story.
    assert_eq!(log.len(), 4);
    log.verify(node.public_key()).unwrap();
    assert_eq!(log.entries()[0].decision, AuditDecision::Allowed);
    assert_eq!(log.entries()[1].decision, AuditDecision::Allowed);
    assert_eq!(log.entries()[2].decision, AuditDecision::Denied(DenyReason::Revoked));
    assert_eq!(log.entries()[3].decision, AuditDecision::Denied(DenyReason::NoCapability));
}
