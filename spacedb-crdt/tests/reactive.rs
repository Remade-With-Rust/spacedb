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
