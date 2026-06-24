//! M5-S3: the signed, content-addressed, append-only audit log.

use spacedb_access::{AuditDecision, AuditError, AuditLog, DenyReason, Did, Identity, Ops, Scope};

fn node() -> Identity {
    Identity::generate("did:mata:node").unwrap()
}

fn scope() -> Scope {
    Scope::Collection("notes".into())
}

#[test]
fn records_and_verifies() {
    let node = node();
    let agent = Did::from("did:agent:bot");
    let mut log = AuditLog::new();
    log.record(&node, 100, &agent, &scope(), Ops::READ, Some([1u8; 16]), AuditDecision::Allowed).unwrap();
    log.record(
        &node,
        101,
        &agent,
        &scope(),
        Ops::WRITE,
        None,
        AuditDecision::Denied(DenyReason::OpNotGranted),
    )
    .unwrap();

    assert_eq!(log.len(), 2);
    log.verify(node.public_key()).unwrap();
    assert_eq!(log.entries()[0].decision, AuditDecision::Allowed);
    assert_eq!(
        log.entries()[1].decision,
        AuditDecision::Denied(DenyReason::OpNotGranted)
    );
}

#[test]
fn tampering_a_field_is_detected() {
    let node = node();
    let agent = Did::from("did:agent:bot");
    let mut log = AuditLog::new();
    log.record(&node, 100, &agent, &scope(), Ops::READ, None, AuditDecision::Allowed).unwrap();
    log.record(&node, 101, &agent, &scope(), Ops::READ, None, AuditDecision::Allowed).unwrap();

    let mut entries = log.entries().to_vec();
    entries[0].at_unix = 999; // rewrite history
    let tampered = AuditLog::from_entries(entries);
    assert!(matches!(
        tampered.verify(node.public_key()),
        Err(AuditError::HashMismatch(0))
    ));
}

#[test]
fn breaking_the_chain_is_detected() {
    let node = node();
    let agent = Did::from("did:agent:bot");
    let mut log = AuditLog::new();
    log.record(&node, 100, &agent, &scope(), Ops::READ, None, AuditDecision::Allowed).unwrap();
    log.record(&node, 101, &agent, &scope(), Ops::READ, None, AuditDecision::Allowed).unwrap();

    let mut entries = log.entries().to_vec();
    entries[1].prev_hash = [0xFF; 32]; // unlink the chain
    let tampered = AuditLog::from_entries(entries);
    assert!(matches!(
        tampered.verify(node.public_key()),
        Err(AuditError::BrokenChain(1))
    ));
}

#[test]
fn a_wrong_node_key_fails_verification() {
    let node = node();
    let other = Identity::generate("did:mata:other").unwrap();
    let agent = Did::from("did:agent:bot");
    let mut log = AuditLog::new();
    log.record(&node, 100, &agent, &scope(), Ops::READ, None, AuditDecision::Allowed).unwrap();
    assert!(matches!(
        log.verify(other.public_key()),
        Err(AuditError::BadSignature(0))
    ));
}
