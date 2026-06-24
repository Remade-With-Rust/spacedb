//! M3-S3: convergence lag from a state-vector diff.

use spacedb_crdt::CrdtDoc;

#[test]
fn ops_behind_counts_missing_then_zero_after_catch_up() {
    let a = CrdtDoc::new(1);
    let b = CrdtDoc::new(2);

    a.set_register("x", &1u64).unwrap();
    a.increment("c", 3);

    assert!(b.ops_behind(&a.state_vector()).unwrap() >= 1, "b is behind a");

    b.apply_update(&a.encode_full()).unwrap();
    assert_eq!(b.ops_behind(&a.state_vector()).unwrap(), 0, "b is caught up");
    assert_eq!(a.ops_behind(&b.state_vector()).unwrap(), 0, "a was never behind b");
}

#[test]
fn a_single_op_is_lag_of_one() {
    let a = CrdtDoc::new(1);
    a.set_register("x", &1u64).unwrap();
    let b = CrdtDoc::new(2);
    assert_eq!(b.ops_behind(&a.state_vector()).unwrap(), 1);
}

#[test]
fn lag_accumulates_across_actors() {
    let a = CrdtDoc::new(1);
    let b = CrdtDoc::new(2);
    let observer = CrdtDoc::new(3);

    a.set_register("a", &1u64).unwrap();
    b.set_register("b", &2u64).unwrap();
    // observer merges a's state but not b's
    observer.apply_update(&a.encode_full()).unwrap();

    assert_eq!(observer.ops_behind(&a.state_vector()).unwrap(), 0, "has all of a");
    assert!(observer.ops_behind(&b.state_vector()).unwrap() >= 1, "still missing b");
}
