//! Deterministic instruction-count driver for authorization and the audit log
//! (callgrind Ir). Keys come from `getrandom`, so a measurement run pins it;
//! ECDSA signing is RFC 6979, so fixed keys give fixed signatures.

use spacedb_access::{
    authorize, authorize_chain, delegate, AccessRequest, AuditDecision, AuditLog, Capability,
    CapabilityChain, Decision, Did, Identity, MemKeyDirectory, Ops, RevocationSet, Scope,
    SignedCapability,
};

const NOW: u64 = 1_000_000_000;
const FAR: u64 = 9_000_000_000;

fn main() {
    let owner = Identity::generate("did:mata:owner").unwrap();
    let agent = Identity::generate("did:agent:assistant").unwrap();
    let helper = Did::from("did:agent:helper");
    let node = Identity::generate("did:mata:node").unwrap();
    let dir = MemKeyDirectory::new();
    dir.publish(&owner).unwrap();
    dir.publish(&agent).unwrap();
    let mut revocations = RevocationSet::new();
    for i in 0..64u8 {
        revocations.revoke([i; 16]);
    }

    let scope = Scope::Collection("notes".into());
    let root = Capability::grant(
        owner.did().clone(),
        agent.did().clone(),
        scope.clone(),
        Ops::READ | Ops::WRITE,
    )
    .unwrap()
    .with_delegation_depth(2)
    .with_expiry(FAR);
    let signed = SignedCapability::sign(root, &owner).unwrap();
    let chain = CapabilityChain::single(signed.clone());
    let sub = Capability::grant(
        agent.did().clone(),
        helper.clone(),
        scope.clone(),
        Ops::READ,
    )
    .unwrap()
    .with_delegation_depth(0)
    .with_expiry(FAR);
    let chain2 = delegate(&chain, sub, &agent).unwrap();

    let (mut allowed, mut denied) = (0u32, 0u32);
    let mut tally = |d: Decision| {
        if d.is_allowed() {
            allowed += 1
        } else {
            denied += 1
        }
    };
    let other = Scope::Collection("secrets".into());
    for i in 0..60 {
        let req = AccessRequest {
            bearer: agent.did(),
            scope: &scope,
            op: Ops::WRITE,
        };
        tally(authorize(&signed, &req, &dir, NOW, &revocations).unwrap());
        // Deny traffic: wrong scope.
        let req = AccessRequest {
            bearer: agent.did(),
            scope: &other,
            op: Ops::READ,
        };
        tally(authorize(&signed, &req, &dir, NOW + i, &revocations).unwrap());
        let req = AccessRequest {
            bearer: &helper,
            scope: &scope,
            op: Ops::READ,
        };
        tally(authorize_chain(&chain2, &req, &dir, NOW, &revocations).unwrap());
    }

    let mut log = AuditLog::new();
    for i in 0..40u64 {
        let decision = if i % 3 == 0 {
            AuditDecision::Denied(spacedb_access::DenyReason::OpNotGranted)
        } else {
            AuditDecision::Allowed
        };
        log.record(
            &node,
            100 + i,
            agent.did(),
            &scope,
            Ops::READ,
            Some([i as u8; 16]),
            decision,
        )
        .unwrap();
    }
    for _ in 0..3 {
        log.verify(node.public_key()).unwrap();
    }

    println!("allowed {allowed} denied {denied} audit {}", log.len());
}
