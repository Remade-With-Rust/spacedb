//! M8-S2: the SDK as a developer uses it — the whole stack through one surface.

use spacedb_sdk::{
    Capability, CrdtType, Database, Did, Identity, Ops, Outcome, RejectReason, Schema, SdkError,
    Scope, SignedCapability, StrongResult, Tier, UnavailableReason,
};

const NOW: u64 = 1_000_000_000;
const FAR: u64 = 9_000_000_000;

fn profile_schema() -> Schema {
    Schema::new("profile")
        .field("bio", CrdtType::Text, Tier::Convergent)
        .field("display_name", CrdtType::Register, Tier::Convergent)
        .field("cursor", CrdtType::Register, Tier::Causal)
        .field("visits", CrdtType::Counter, Tier::Convergent)
        .field("tags", CrdtType::Set, Tier::Convergent)
        .field("username", CrdtType::Register, Tier::Strong)
}

/// A home replica with the profile schema and the owner's key published.
fn setup() -> (Database, Identity, Did) {
    let owner = Identity::generate("did:mata:owner").unwrap();
    let mut db = Database::open(Identity::generate("did:mata:home-1").unwrap());
    db.register_identity(&owner).unwrap();
    db.set_clock(NOW);
    db.define(profile_schema());
    (db, owner, Did::from("did:agent:assistant"))
}

fn grant(owner: &Identity, bearer: &Did, ops: Ops, budget: u64) -> SignedCapability {
    let cap = Capability::grant(
        owner.did().clone(),
        bearer.clone(),
        Scope::Collection("profile".into()),
        ops,
    )
    .unwrap()
    .with_expiry(FAR)
    .with_budget(budget);
    SignedCapability::sign(cap, owner).unwrap()
}

// ── offline-first reads/writes, honest state, per-field CRDT types ───────────

#[test]
fn offline_writes_are_locally_durable_and_read_back() {
    let (mut db, owner, agent) = setup();
    let mut s = db.session(grant(&owner, &agent, Ops::READ | Ops::WRITE, 1_000_000));

    // a convergent register: written Local, read back Committed(Convergent)
    assert_eq!(
        db.put_register(&mut s, "profile", "display_name", "Ada").unwrap(),
        Outcome::Local
    );
    let (value, outcome) = db.read_register(&mut s, "profile", "display_name").unwrap();
    assert_eq!(value, Some("Ada".to_string()));
    assert_eq!(outcome, Outcome::Committed(Tier::Convergent));

    // each CRDT type behaves by its semantics
    db.increment(&mut s, "profile", "visits", 5).unwrap();
    db.increment(&mut s, "profile", "visits", -2).unwrap();
    assert_eq!(db.counter("profile", "visits"), 3);

    db.append_text(&mut s, "profile", "bio", "hello ").unwrap();
    db.append_text(&mut s, "profile", "bio", "world").unwrap();
    assert_eq!(db.text("profile", "bio"), "hello world");

    db.add_to_set(&mut s, "profile", "tags", "rust").unwrap();
    db.add_to_set(&mut s, "profile", "tags", "db").unwrap();
    let mut tags = db.set_members("profile", "tags");
    tags.sort();
    assert_eq!(tags, vec!["db".to_string(), "rust".to_string()]);
}

#[test]
fn a_causal_field_reads_its_own_write() {
    let (mut db, owner, agent) = setup();
    let mut s = db.session(grant(&owner, &agent, Ops::READ | Ops::WRITE, 1_000_000));

    assert_eq!(
        db.put_register(&mut s, "profile", "cursor", "page-42").unwrap(),
        Outcome::Local
    );
    // a causal read of one's own write is up to date
    let (value, outcome) = db.read_register(&mut s, "profile", "cursor").unwrap();
    assert_eq!(value, Some("page-42".to_string()));
    assert_eq!(outcome, Outcome::Committed(Tier::Causal));
}

#[test]
fn the_wrong_crdt_op_for_a_field_is_rejected() {
    let (mut db, owner, agent) = setup();
    let mut s = db.session(grant(&owner, &agent, Ops::WRITE, 1_000_000));
    // visits is a Counter, not a Register
    let err = db.put_register(&mut s, "profile", "visits", "x").unwrap_err();
    assert!(matches!(err, SdkError::WrongType { .. }));
    // and a strong field can't be set via a plain put
    let err = db.put_register(&mut s, "profile", "username", "x").unwrap_err();
    assert!(matches!(err, SdkError::StrongFieldNeedsClaim(_)));
}

// ── mID authorization ────────────────────────────────────────────────────────

#[test]
fn an_op_outside_the_granted_scope_is_denied() {
    let (mut db, owner, agent) = setup();
    // a capability for a different collection
    let cap = Capability::grant(
        owner.did().clone(),
        agent.clone(),
        Scope::Collection("billing".into()),
        Ops::WRITE,
    )
    .unwrap()
    .with_expiry(FAR)
    .with_budget(1_000_000);
    let mut s = db.session(SignedCapability::sign(cap, &owner).unwrap());

    let err = db.put_register(&mut s, "profile", "display_name", "x").unwrap_err();
    assert!(matches!(err, SdkError::Denied(_)));
}

#[test]
fn a_revoked_capability_stops_working() {
    let (mut db, owner, agent) = setup();
    let cap = grant(&owner, &agent, Ops::WRITE, 1_000_000);
    let cap_id = cap.capability.id;
    let mut s = db.session(cap);

    db.put_register(&mut s, "profile", "display_name", "before").unwrap();
    db.revoke(cap_id); // mID kill-switch
    let err = db.put_register(&mut s, "profile", "display_name", "after").unwrap_err();
    assert!(matches!(err, SdkError::Denied(_)));
}

// ── budget (the agent spends from its own capability) ────────────────────────

#[test]
fn an_agent_cannot_exceed_its_capability_budget() {
    let (mut db, owner, agent) = setup();
    let cost = db.write_cost();
    // budget for exactly three writes
    let mut s = db.session(grant(&owner, &agent, Ops::WRITE, cost * 3));

    for _ in 0..3 {
        db.put_register(&mut s, "profile", "display_name", "v").unwrap();
    }
    assert_eq!(s.budget_remaining(), 0);

    let err = db.put_register(&mut s, "profile", "display_name", "v4").unwrap_err();
    assert!(matches!(err, SdkError::OverBudget { .. }));
}

// ── strong tier through the SDK ──────────────────────────────────────────────

#[test]
fn strong_uniqueness_is_enforced_and_fails_safe_under_partition() {
    let (mut db, owner, agent) = setup();
    let mut s = db.session(grant(&owner, &agent, Ops::READ | Ops::WRITE, 1_000_000));

    // claim a globally-unique username
    assert_eq!(
        db.claim_unique(&mut s, "profile", "username", "cooluser").unwrap(),
        StrongResult::Committed
    );
    assert_eq!(
        db.unique_owner("profile", "username", "cooluser"),
        Some("did:agent:assistant".to_string())
    );

    // a different agent can't take the same username
    let bob = Did::from("did:agent:bob");
    let mut bs = db.session(grant(&owner, &bob, Ops::WRITE, 1_000_000));
    assert_eq!(
        db.claim_unique(&mut bs, "profile", "username", "cooluser").unwrap(),
        StrongResult::Rejected(RejectReason::AlreadyClaimed)
    );

    // under partition the strong tier fails safe — Unavailable, commits nothing
    db.quorum_partition("q1");
    db.quorum_partition("q2");
    assert_eq!(
        db.claim_unique(&mut s, "profile", "username", "fresh").unwrap(),
        StrongResult::Unavailable(UnavailableReason::QuorumUnreachable)
    );
    assert_eq!(db.unique_owner("profile", "username", "fresh"), None); // nothing committed

    // healed, it resumes
    db.quorum_heal("q1");
    db.quorum_heal("q2");
    assert_eq!(
        db.claim_unique(&mut s, "profile", "username", "fresh").unwrap(),
        StrongResult::Committed
    );
}

// ── reactive + offline sync ──────────────────────────────────────────────────

#[test]
fn a_watcher_observes_local_changes() {
    let (mut db, owner, agent) = setup();
    let mut s = db.session(grant(&owner, &agent, Ops::WRITE, 1_000_000));

    let watcher = db.watch("profile");
    let _ = watcher.drain_changed(); // establish a baseline
    db.put_register(&mut s, "profile", "display_name", "x").unwrap();
    assert!(watcher.drain_changed());
    assert!(!watcher.drain_changed()); // nothing new since the last drain
}

#[test]
fn two_replicas_converge_through_offline_sync() {
    // replica A writes offline
    let (mut a, owner, agent) = setup();
    let mut sa = a.session(grant(&owner, &agent, Ops::WRITE, 1_000_000));
    a.put_register(&mut sa, "profile", "display_name", "Grace").unwrap();
    a.increment(&mut sa, "profile", "visits", 7).unwrap();

    // replica B is a different home that has never talked to A
    let mut b = Database::open(Identity::generate("did:mata:home-2").unwrap());
    b.register_identity(&owner).unwrap();
    b.set_clock(NOW);
    b.define(profile_schema());

    // hand A's exported state to B (no network involved)
    b.import("profile", &a.export("profile")).unwrap();

    assert_eq!(b.counter("profile", "visits"), 7);
    let mut sb = b.session(grant(&owner, &agent, Ops::READ, 1_000_000));
    let (value, _) = b.read_register(&mut sb, "profile", "display_name").unwrap();
    assert_eq!(value, Some("Grace".to_string()));
}
