//! M3-S3: replica-role scaffolding.

use std::collections::BTreeSet;

use spacedb_replica::{ReplicaRole, SubsetSpec};

#[test]
fn full_holds_everything_and_serves_reads() {
    let role = ReplicaRole::Full;
    assert!(role.holds("any-doc"));
    assert!(role.serves_reads());
}

#[test]
fn buyer_only_holds_nothing_and_does_not_serve() {
    let role = ReplicaRole::BuyerOnly;
    assert!(!role.holds("any-doc"));
    assert!(!role.serves_reads());
}

#[test]
fn partial_cache_holds_only_its_subset() {
    let ids: BTreeSet<String> = ["alpha", "beta"].iter().map(|s| s.to_string()).collect();
    let role = ReplicaRole::PartialCache(SubsetSpec::DocIds(ids));
    assert!(role.holds("alpha"));
    assert!(!role.holds("gamma"));
    assert!(role.serves_reads());

    let mirror = ReplicaRole::PartialCache(SubsetSpec::All);
    assert!(mirror.holds("anything"));
}
