//! M3-S1: live convergence and partition recovery between two replicas, over the
//! in-process transport. This is the M3 ship criterion proven in one process: a
//! write on one replica reaches the other, and a partition heals with no lost
//! writes.

use spacedb_crdt::CrdtDoc;
use spacedb_replica::{connected_pair, InProcessTransport, SyncSession};

/// Drive both sessions to quiescence: announce current frontiers, then pump until
/// no further state-vector answers or merges happen.
fn reconcile(a: &SyncSession<InProcessTransport>, b: &SyncSession<InProcessTransport>) {
    a.announce().unwrap();
    b.announce().unwrap();
    for _ in 0..16 {
        let progressed = a.pump().unwrap() + b.pump().unwrap();
        if progressed == 0 {
            break;
        }
    }
}

fn pair() -> (SyncSession<InProcessTransport>, SyncSession<InProcessTransport>, spacedb_replica::Link) {
    let (ta, tb, link) = connected_pair();
    let a = SyncSession::new(CrdtDoc::new(10), ta);
    let b = SyncSession::new(CrdtDoc::new(20), tb);
    (a, b, link)
}

#[test]
fn a_write_propagates_to_the_peer() {
    let (a, b, _link) = pair();
    a.doc().set_register("title", &"hello".to_string()).unwrap();
    a.doc().increment("views", 5);
    reconcile(&a, &b);
    assert_eq!(b.doc().get_register::<String>("title").unwrap(), Some("hello".to_string()));
    assert_eq!(b.doc().counter("views"), 5);
}

#[test]
fn concurrent_edits_converge_bidirectionally() {
    let (a, b, _link) = pair();
    // both edit before any sync
    a.doc().set_register("status", &"from-a".to_string()).unwrap();
    a.doc().increment("n", 3);
    b.doc().set_register("status", &"from-b".to_string()).unwrap();
    b.doc().increment("n", 5);

    reconcile(&a, &b);

    // counter merges by sum; register resolves to the same LWW winner
    assert_eq!(a.doc().counter("n"), 8);
    assert_eq!(b.doc().counter("n"), 8);
    assert_eq!(
        a.doc().get_register::<String>("status").unwrap(),
        b.doc().get_register::<String>("status").unwrap()
    );
}

#[test]
fn incremental_change_after_initial_sync_propagates() {
    let (a, b, _link) = pair();
    a.doc().set_register("k", &1u64).unwrap();
    reconcile(&a, &b);
    assert_eq!(b.doc().get_register::<u64>("k").unwrap(), Some(1));

    // a further local change reaches the peer on the next reconcile
    a.doc().set_register("k", &2u64).unwrap();
    a.doc().text_push("body", "hi");
    reconcile(&a, &b);
    assert_eq!(b.doc().get_register::<u64>("k").unwrap(), Some(2));
    assert_eq!(b.doc().text("body"), "hi");
}

#[test]
fn partition_heals_with_no_lost_writes() {
    let (a, b, link) = pair();

    // establish a shared baseline
    a.doc().set_register("shared", &"base".to_string()).unwrap();
    reconcile(&a, &b);
    assert_eq!(b.doc().get_register::<String>("shared").unwrap(), Some("base".to_string()));

    // PARTITION: the link is cut; both sides keep working offline
    link.partition();
    assert!(!link.is_connected());

    a.doc().increment("n", 10);
    a.doc().set_register("only_a", &"a".to_string()).unwrap();
    a.doc().set_add("tags", "alpha");
    a.doc().text_push("log", "A-wrote ");

    b.doc().increment("n", 7);
    b.doc().set_register("only_b", &"b".to_string()).unwrap();
    b.doc().set_add("tags", "beta");
    b.doc().text_push("log", "B-wrote ");

    // sends during the partition are dropped; nothing crosses
    a.announce().unwrap();
    b.announce().unwrap();
    assert_eq!(a.pump().unwrap(), 0);
    assert_eq!(b.pump().unwrap(), 0);
    // each side still only sees its own offline writes
    assert_eq!(a.doc().counter("n"), 10);
    assert_eq!(b.doc().counter("n"), 7);

    // HEAL: reconnect and reconcile
    link.heal();
    reconcile(&a, &b);

    // no lost writes: every offline edit from both sides survived and converged
    assert_eq!(a.doc().counter("n"), 17, "counter is the sum of both sides' offline increments");
    assert_eq!(b.doc().counter("n"), 17);

    for s in [&a, &b] {
        assert_eq!(s.doc().get_register::<String>("only_a").unwrap(), Some("a".to_string()));
        assert_eq!(s.doc().get_register::<String>("only_b").unwrap(), Some("b".to_string()));
        assert_eq!(s.doc().set_members("tags"), vec!["alpha".to_string(), "beta".to_string()]);
        assert_eq!(s.doc().text("log"), a.doc().text("log")); // same converged interleaving
        assert!(s.doc().text("log").contains("A-wrote"));
        assert!(s.doc().text("log").contains("B-wrote"));
    }
}
