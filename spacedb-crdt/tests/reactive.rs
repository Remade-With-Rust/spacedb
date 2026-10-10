//! M3-S2: reactive queries over a document's change stream.

use spacedb_crdt::{CrdtDoc, ReactiveQuery};

#[test]
fn watcher_fires_on_mutation_and_drains() {
    let d = CrdtDoc::new(1);
    let w = d.watch();
    assert!(!w.drain_changed(), "no change yet");

    d.set_register("x", &1u64).unwrap();
    assert!(w.drain_changed(), "a mutation is a change");
    assert!(!w.drain_changed(), "the change was drained");

    d.increment("c", 3);
    assert!(w.drain_changed());
}

#[test]
fn reads_do_not_register_as_changes() {
    let d = CrdtDoc::new(1);
    d.set_register("x", &1u64).unwrap();
    let w = d.watch();
    // pure reads must not bump the revision
    let _ = d.get_register::<u64>("x").unwrap();
    let _ = d.counter("c");
    let _ = d.set_members("tags");
    let _ = d.text("body");
    assert!(!w.drain_changed(), "reads must not count as changes");
}

#[test]
fn reactive_query_emits_only_when_its_result_changes() {
    let d = CrdtDoc::new(1);
    d.set_register("title", &"a".to_string()).unwrap();

    let mut q = ReactiveQuery::new(&d, |doc| doc.get_register::<String>("title").unwrap());
    assert_eq!(q.current().clone(), Some("a".to_string()));
    assert_eq!(q.poll(&d), None, "no change since construction");

    // a change that affects the query result -> emit the new result
    d.set_register("title", &"b".to_string()).unwrap();
    assert_eq!(q.poll(&d), Some(Some("b".to_string())));
    assert_eq!(q.poll(&d), None, "nothing new since");

    // a change that does NOT affect this query's result -> no emission
    d.set_register("unrelated", &"z".to_string()).unwrap();
    assert_eq!(q.poll(&d), None, "the watched result is unchanged");
}

#[test]
fn reactive_counter_query() {
    let d = CrdtDoc::new(1);
    let mut q = ReactiveQuery::new(&d, |doc| doc.counter("n"));
    assert_eq!(*q.current(), 0);

    d.increment("n", 5);
    assert_eq!(q.poll(&d), Some(5));

    d.increment("n", 2);
    assert_eq!(q.poll(&d), Some(7));
    assert_eq!(q.poll(&d), None);
}

/// An unlogged document keeps no local-update log but is otherwise the same
/// document: its revision moves and its watchers fire on every change.
#[test]
fn an_unlogged_document_still_reports_changes() {
    let d = CrdtDoc::new_unlogged(1);
    let w = d.watch();
    let before = d.revision();
    d.increment("n", 1);
    d.set_register("x", &1u64).unwrap();
    assert!(d.revision() > before);
    assert!(w.drain_changed());
    assert!(d.take_local_updates().is_empty(), "nothing is logged");
    assert_eq!(d.counter("n"), 1);

    // Its full state still seeds a logged replica exactly.
    let peer = CrdtDoc::new(2);
    peer.apply_update(&d.encode_full()).unwrap();
    assert_eq!(peer.counter("n"), 1);
}

/// `state_vector` is reused only while the document is unchanged: a local
/// write or a merged remote update yields a new vector, equal to what a fresh
/// computation gives.
#[test]
fn the_state_vector_follows_every_change() {
    let a = CrdtDoc::new(1);
    let b = CrdtDoc::new(2);
    let sv0 = a.state_vector();
    assert_eq!(a.state_vector(), sv0, "unchanged document, same vector");
    a.increment("n", 1);
    let sv1 = a.state_vector();
    assert_ne!(sv1, sv0, "a local write moves the vector");
    b.increment("n", 5);
    a.apply_update(&b.encode_full()).unwrap();
    let sv2 = a.state_vector();
    assert_ne!(sv2, sv1, "a merged remote update moves the vector");
    // What a fresh document with the same history reports.
    let c = CrdtDoc::new(3);
    c.apply_update(&a.encode_full()).unwrap();
    assert_eq!(c.ops_behind(&sv2).unwrap(), 0);
    assert_eq!(a.ops_behind(&c.state_vector()).unwrap(), 0);
}
