//! Deterministic instruction-count driver for the SDK's per-op path
//! (callgrind Ir): every read and write goes through field lookup,
//! authorization, metering and the CRDT. Keys come from `getrandom` and yrs
//! client ids from fastrand; a measurement run pins both.

use spacedb_sdk::{
    Capability, CrdtType, Database, Did, Identity, Ops, Schema, Scope, SignedCapability, Tier,
};

const NOW: u64 = 1_000_000_000;
const FAR: u64 = 9_000_000_000;

fn main() {
    fastrand::seed(5);
    let owner = Identity::generate("did:mata:owner").unwrap();
    let mut db = Database::open(Identity::generate("did:mata:home-1").unwrap());
    db.register_identity(&owner).unwrap();
    db.set_clock(NOW);
    db.define(
        Schema::new("profile")
            .field("bio", CrdtType::Text, Tier::Convergent)
            .field("display_name", CrdtType::Register, Tier::Convergent)
            .field("cursor", CrdtType::Register, Tier::Causal)
            .field("visits", CrdtType::Counter, Tier::Convergent)
            .field("tags", CrdtType::Set, Tier::Convergent)
            .field("username", CrdtType::Register, Tier::Strong),
    );
    let agent = Did::from("did:agent:assistant");
    let cap = Capability::grant(
        owner.did().clone(),
        agent.clone(),
        Scope::Collection("profile".into()),
        Ops::READ | Ops::WRITE,
    )
    .unwrap()
    .with_expiry(FAR)
    .with_budget(1_000_000_000);
    let mut s = db.session(SignedCapability::sign(cap, &owner).unwrap());

    let mut check = 0i64;
    for i in 0..60 {
        db.put_register(&mut s, "profile", "display_name", &format!("Ada {i}"))
            .unwrap();
        db.put_register(&mut s, "profile", "cursor", &format!("{i}"))
            .unwrap();
        db.increment(&mut s, "profile", "visits", 1).unwrap();
        db.add_to_set(&mut s, "profile", "tags", ["rust", "db", "crdt"][i % 3])
            .unwrap();
        if i % 4 == 0 {
            db.append_text(&mut s, "profile", "bio", "x").unwrap();
        }
        let (v, _) = db.read_register(&mut s, "profile", "cursor").unwrap();
        check += v.map(|v| v.len() as i64).unwrap_or(0);
        if i % 10 == 0 {
            let _ = db.claim_unique(&mut s, "profile", "username", &format!("user{}", i % 3));
        }
    }
    check += db.counter("profile", "visits") + db.set_members("profile", "tags").len() as i64;
    check += db.text("profile", "bio").len() as i64;
    println!("check {check}");
}
