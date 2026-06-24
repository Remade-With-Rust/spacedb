//! M3-S3: honest read freshness — a replica never silently mistakes a stale or
//! partitioned read for a current one.

use spacedb_crdt::CrdtDoc;
use spacedb_replica::{connected_pair, Freshness, InProcessTransport, SyncSession};

fn reconcile(a: &SyncSession<InProcessTransport>, b: &SyncSession<InProcessTransport>) {
    a.announce().unwrap();
    b.announce().unwrap();
    for _ in 0..16 {
        if a.pump().unwrap() + b.pump().unwrap() == 0 {
            break;
        }
    }
}

fn pair() -> (SyncSession<InProcessTransport>, SyncSession<InProcessTransport>, spacedb_replica::Link) {
    let (ta, tb, link) = connected_pair();
    (
        SyncSession::new(CrdtDoc::new(10), ta),
        SyncSession::new(CrdtDoc::new(20), tb),
        link,
    )
}

#[test]
fn unsynced_until_first_reconcile_then_live() {
    let (a, b, _link) = pair();
    assert_eq!(b.freshness(), Freshness::Unsynced, "no peer frontier seen yet");
    assert_eq!(b.lag(), 0);

    a.doc().set_register("x", &1u64).unwrap();
    reconcile(&a, &b);

    assert_eq!(b.freshness(), Freshness::Live);
    assert_eq!(b.lag(), 0);
}

#[test]
fn reports_stale_with_lag_then_live_after_catch_up() {
    let (a, b, _link) = pair();
    a.doc().set_register("a", &1u64).unwrap();
    a.doc().set_register("b", &2u64).unwrap();
    a.doc().increment("c", 1);

    // both announce; b processes a's frontier first — it now knows it is behind
    a.announce().unwrap();
    b.announce().unwrap();
    b.pump().unwrap();
    let lag = b.lag();
    assert!(lag > 0, "b knows it is behind a by {lag} ops");
    assert_eq!(b.freshness(), Freshness::Stale { lag_ops: lag });

    // a replies with its delta; b applies it and catches up
    a.pump().unwrap();
    b.pump().unwrap();
    assert_eq!(b.lag(), 0);
    assert_eq!(b.freshness(), Freshness::Live);
}

#[test]
fn partitioned_link_reads_as_partitioned() {
    let (a, b, link) = pair();
    a.doc().set_register("x", &1u64).unwrap();
    reconcile(&a, &b);
    assert_eq!(b.freshness(), Freshness::Live);

    link.partition();
    assert_eq!(b.freshness(), Freshness::Partitioned, "a down link is not silently live");

    link.heal();
    assert_eq!(b.freshness(), Freshness::Live);
}
