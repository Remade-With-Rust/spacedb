//! M7-S1: tier annotation, the honesty contract, and the Causal+ session.

use spacedb_consistency::{CausalSession, ConsistencySchema, Outcome, Tier};
use spacedb_crdt::CrdtDoc;

#[test]
fn the_schema_annotates_tiers_per_field_defaulting_to_convergent() {
    let schema = ConsistencySchema::new()
        .with_field("username", Tier::Strong) // uniqueness
        .with_field("cursor", Tier::Causal); // read-your-writes

    assert_eq!(schema.tier_of("username"), Tier::Strong);
    assert_eq!(schema.tier_of("cursor"), Tier::Causal);
    assert_eq!(schema.tier_of("bio"), Tier::Convergent); // the 95% default
    assert_eq!(schema.default_tier(), Tier::Convergent);
}

#[test]
fn the_outcome_reports_the_level_honestly() {
    assert!(Outcome::Committed(Tier::Causal).is_committed());
    assert_eq!(Outcome::Committed(Tier::Strong).tier(), Some(Tier::Strong));
    assert!(Outcome::Local.is_available());
    assert!(!Outcome::Local.is_committed());
    assert!(Outcome::Stale { lag_ops: 4 }.is_available());
    assert!(!Outcome::Unavailable(spacedb_consistency::UnavailableReason::Partition).is_available());
}

#[test]
fn a_causal_session_reads_its_own_writes() {
    let doc = CrdtDoc::new(1);
    let mut session = CausalSession::new();

    doc.set_register("note", &"hi".to_string()).unwrap();
    assert_eq!(session.record_write(&doc), Outcome::Local);

    // a read of the same replica is up to date and sees the write
    assert_eq!(session.read(&doc), Outcome::Committed(Tier::Causal));
    assert_eq!(doc.get_register::<String>("note").unwrap(), Some("hi".to_string()));
}

#[test]
fn reads_are_monotonic_a_lagging_replica_is_reported_stale() {
    // replica A is ahead; replica B is empty
    let a = CrdtDoc::new(1);
    a.set_register("x", &1u64).unwrap();
    a.increment("c", 3);
    let b = CrdtDoc::new(2);

    let mut session = CausalSession::new();

    // read A: up to date, session now has observed A's frontier
    assert_eq!(session.read(&a), Outcome::Committed(Tier::Causal));

    // read B (behind what we've seen): honestly stale, not silently served
    match session.read(&b) {
        Outcome::Stale { lag_ops } => assert!(lag_ops >= 1),
        other => panic!("expected Stale, got {other:?}"),
    }

    // once B catches up, the read is served again (monotonic, never backwards)
    b.apply_update(&a.encode_full()).unwrap();
    assert_eq!(session.read(&b), Outcome::Committed(Tier::Causal));
}

#[test]
fn a_fresh_session_reads_any_replica_as_current() {
    // a brand-new session has observed nothing, so even an empty replica is a
    // valid causal view for it (it just can't have seen anything newer)
    let b = CrdtDoc::new(2);
    let mut session = CausalSession::new();
    assert_eq!(session.read(&b), Outcome::Committed(Tier::Causal));
}

#[test]
fn read_your_writes_holds_across_a_sync_to_another_replica() {
    // write on A, sync to B, then a session that saw A's write reads B and still
    // sees it (no regression)
    let a = CrdtDoc::new(1);
    let b = CrdtDoc::new(2);
    a.set_register("k", &7u64).unwrap();

    let mut session = CausalSession::new();
    session.record_write(&a); // session has observed A's write

    // B hasn't synced yet -> stale
    assert!(matches!(session.read(&b), Outcome::Stale { .. }));

    // sync A -> B, now B is current for the session
    b.apply_update(&a.encode_full()).unwrap();
    assert_eq!(session.read(&b), Outcome::Committed(Tier::Causal));
    assert_eq!(b.get_register::<u64>("k").unwrap(), Some(7));
}
