//! M7-S2: the Strong (quorum) tier — linearizable, and fail-safe under partition.

use spacedb_consistency::{
    Outcome, QuorumGroup, RejectReason, StrongResult, Tier, UnavailableReason,
};

fn group() -> QuorumGroup {
    QuorumGroup::new(["m0", "m1", "m2"])
}

#[test]
fn a_unique_username_is_first_claim_wins() {
    let mut q = group();
    assert_eq!(q.claim_unique("cooluser", b"alice"), StrongResult::Committed);

    // bob tries the same username — refused, not double-claimed
    assert_eq!(
        q.claim_unique("cooluser", b"bob"),
        StrongResult::Rejected(RejectReason::AlreadyClaimed)
    );
    assert_eq!(q.read("cooluser").unwrap().0, Some(b"alice".to_vec()));

    // a different username is free
    assert_eq!(q.claim_unique("otheruser", b"bob"), StrongResult::Committed);
}

#[test]
fn the_last_seat_is_never_oversold() {
    let mut q = group();
    assert_eq!(q.init_seats("concert", 1), StrongResult::Committed);

    assert_eq!(q.acquire_seat("concert"), StrongResult::Committed); // takes the last seat
    assert_eq!(q.seats_remaining("concert").unwrap(), 0);
    assert_eq!(
        q.acquire_seat("concert"),
        StrongResult::Rejected(RejectReason::Exhausted) // sold out, not negative
    );
    assert_eq!(q.seats_remaining("concert").unwrap(), 0);
}

#[test]
fn seats_drain_exactly_with_no_oversell() {
    let mut q = group();
    q.init_seats("seats", 3);
    assert!(q.acquire_seat("seats").is_committed());
    assert!(q.acquire_seat("seats").is_committed());
    assert!(q.acquire_seat("seats").is_committed());
    assert_eq!(
        q.acquire_seat("seats"),
        StrongResult::Rejected(RejectReason::Exhausted)
    );
    assert_eq!(q.seats_remaining("seats").unwrap(), 0);
}

#[test]
fn a_concurrent_cas_lets_exactly_one_win() {
    let mut q = group();
    // two writers both observe version 0
    let (_, v) = q.read("k").unwrap();
    assert_eq!(v, 0);
    assert_eq!(q.cas("k", 0, b"first".to_vec()), StrongResult::Committed);
    // the second writer's stale CAS loses the race
    assert_eq!(
        q.cas("k", 0, b"second".to_vec()),
        StrongResult::Rejected(RejectReason::VersionConflict)
    );
    assert_eq!(q.read("k").unwrap().0, Some(b"first".to_vec()));
}

#[test]
fn a_partition_without_quorum_fails_safe() {
    let mut q = group();
    assert_eq!(q.claim_unique("cooluser", b"alice"), StrongResult::Committed);
    q.init_seats("concert", 1);

    // partition: only 1 of 3 reachable — below majority
    q.partition("m1");
    q.partition("m2");
    assert_eq!(q.online_count(), 1);

    // strong ops refuse rather than commit divergently
    assert_eq!(
        q.claim_unique("cooluser2", b"bob"),
        StrongResult::Unavailable(UnavailableReason::QuorumUnreachable)
    );
    assert_eq!(
        q.acquire_seat("concert"),
        StrongResult::Unavailable(UnavailableReason::QuorumUnreachable)
    );
    assert!(q.read("cooluser").is_err()); // can't even read without a quorum
    assert!(q.seats_remaining("concert").is_err());

    // heal: prior state is intact (no divergence) and ops resume
    q.heal("m1");
    q.heal("m2");
    assert_eq!(q.read("cooluser").unwrap().0, Some(b"alice".to_vec()));
    assert_eq!(q.seats_remaining("concert").unwrap(), 1); // never decremented during the partition
    assert_eq!(q.claim_unique("cooluser2", b"bob"), StrongResult::Committed);
}

#[test]
fn commits_continue_on_a_majority_side_during_a_single_member_partition() {
    let mut q = group();
    // one member partitioned away; the remaining 2 are still a majority
    q.partition("m2");
    assert_eq!(q.online_count(), 2);
    assert_eq!(q.claim_unique("cooluser", b"alice"), StrongResult::Committed);

    // the lagging member catches up on heal (read-repair via the highest version)
    q.heal("m2");
    assert_eq!(q.read("cooluser").unwrap().0, Some(b"alice".to_vec()));
}

#[test]
fn strong_results_map_to_the_honesty_contract() {
    assert_eq!(StrongResult::Committed.consistency(), Outcome::Committed(Tier::Strong));
    assert_eq!(
        StrongResult::Rejected(RejectReason::AlreadyClaimed).consistency(),
        Outcome::Committed(Tier::Strong) // a reached quorum gave a definitive answer
    );
    assert_eq!(
        StrongResult::Unavailable(UnavailableReason::QuorumUnreachable).consistency(),
        Outcome::Unavailable(UnavailableReason::QuorumUnreachable)
    );
}
