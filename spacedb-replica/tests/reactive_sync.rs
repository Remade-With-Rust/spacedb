//! M3-S2: a reactive query re-fires as a *remote* change converges — the
//! "re-render as the mesh converges" behaviour, end to end over the transport.

use spacedb_crdt::{CrdtDoc, ReactiveQuery};
use spacedb_replica::{connected_pair, InProcessTransport, SyncSession};

fn reconcile(a: &SyncSession<InProcessTransport>, b: &SyncSession<InProcessTransport>) {
    a.announce().unwrap();
    b.announce().unwrap();
    for _ in 0..16 {
        if a.pump().unwrap() + b.pump().unwrap() == 0 {
            break;
        }
    }
}

#[test]
fn reactive_query_fires_when_a_remote_change_converges() {
    let (ta, tb, _link) = connected_pair();
    let a = SyncSession::new(CrdtDoc::new(10), ta);
    let b = SyncSession::new(CrdtDoc::new(20), tb);

    // b watches a query over its own replica
    let mut headline =
        ReactiveQuery::new(b.doc(), |doc| doc.get_register::<String>("headline").unwrap());
    assert_eq!(b.doc().get_register::<String>("headline").unwrap(), None);
    assert_eq!(headline.poll(b.doc()), None);

    // a publishes; the change flows over the transport and converges on b
    a.doc().set_register("headline", &"breaking".to_string()).unwrap();
    reconcile(&a, &b);

    // b's reactive query now observes the converged value
    assert_eq!(
        headline.poll(b.doc()),
        Some(Some("breaking".to_string())),
        "a remote change must re-fire b's reactive query"
    );
    assert_eq!(headline.poll(b.doc()), None, "nothing new after convergence");
}

#[test]
fn reactive_counter_reflects_remote_increments() {
    let (ta, tb, _link) = connected_pair();
    let a = SyncSession::new(CrdtDoc::new(10), ta);
    let b = SyncSession::new(CrdtDoc::new(20), tb);

    let mut total = ReactiveQuery::new(b.doc(), |doc| doc.counter("likes"));
    a.doc().increment("likes", 4);
    reconcile(&a, &b);
    assert_eq!(total.poll(b.doc()), Some(4));

    // b's own local increment also re-fires, summed with the remote one
    b.doc().increment("likes", 3);
    assert_eq!(total.poll(b.doc()), Some(7));
}
