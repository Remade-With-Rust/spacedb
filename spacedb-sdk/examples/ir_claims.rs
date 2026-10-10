//! Deterministic instruction-count driver for strong-tier uniqueness claims and
//! their owner lookups (`claim_unique`, `unique_owner`). The owner tally is the
//! anchor.

use spacedb_sdk::{
    Capability, CrdtType, Database, Did, Identity, Ops, Schema, Scope, SignedCapability, Tier,
};

fn main() {
    fastrand::seed(9);
    let owner = Identity::generate("did:mata:owner").unwrap();
    let mut db = Database::open(Identity::generate("did:mata:home-1").unwrap());
    db.register_identity(&owner).unwrap();
    db.set_clock(1_000_000_000);
    db.define(Schema::new("users").field("username", CrdtType::Register, Tier::Strong));
    let cap = Capability::grant(
        owner.did().clone(),
        Did::from("did:agent:a"),
        Scope::Collection("users".into()),
        Ops::READ | Ops::WRITE,
    )
    .unwrap()
    .with_expiry(9_000_000_000)
    .with_budget(1_000_000_000);
    let mut s = db.session(SignedCapability::sign(cap, &owner).unwrap());
    let mut owned = 0usize;
    for i in 0..300 {
        let _ = db.claim_unique(&mut s, "users", "username", &format!("name{}", i % 40));
        for j in 0..10 {
            owned += db
                .unique_owner("users", "username", &format!("name{}", (i + j) % 50))
                .map(|o| o.len())
                .unwrap_or(0);
        }
    }
    println!("owned {owned}");
}
