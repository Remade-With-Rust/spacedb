//! M4-S4 (CRDT side): update-log compaction.

use spacedb_crdt::{compact_updates, CrdtDoc};

#[test]
fn compacting_a_long_update_log_shrinks_it_but_preserves_state() {
    // a register overwritten many times produces a long log of small updates
    let doc = CrdtDoc::new(1);
    let mut log: Vec<Vec<u8>> = Vec::new();
    let mut seen = doc.state_vector();
    for i in 0..300i64 {
        doc.set_register("v", &i).unwrap();
        log.push(doc.encode_update_since(&seen).unwrap());
        seen = doc.state_vector();
    }

    let merged = compact_updates(&log).unwrap();
    let total: usize = log.iter().map(|u| u.len()).sum();
    assert!(merged.len() < total, "compaction should shrink the log");

    // applying the merged blob reproduces exactly the same state as replaying
    // every original update
    let from_merged = CrdtDoc::new(2);
    from_merged.apply_update(&merged).unwrap();
    assert_eq!(from_merged.get_register::<i64>("v").unwrap(), Some(299));

    let from_log = CrdtDoc::new(3);
    for u in &log {
        from_log.apply_update(u).unwrap();
    }
    assert_eq!(
        from_merged.get_register::<i64>("v").unwrap(),
        from_log.get_register::<i64>("v").unwrap()
    );
}

#[test]
fn compaction_is_order_independent() {
    let doc = CrdtDoc::new(1);
    doc.set_register("a", &"x".to_string()).unwrap();
    let u1 = doc.encode_full();
    doc.increment("c", 5);
    let u2 = doc.encode_update_since(&{
        let d = CrdtDoc::new(9);
        d.apply_update(&u1).unwrap();
        d.state_vector()
    })
    .unwrap();

    let forward = compact_updates(&[u1.clone(), u2.clone()]).unwrap();
    let reverse = compact_updates(&[u2, u1]).unwrap();

    let a = CrdtDoc::new(4);
    a.apply_update(&forward).unwrap();
    let b = CrdtDoc::new(5);
    b.apply_update(&reverse).unwrap();

    assert_eq!(a.get_register::<String>("a").unwrap(), Some("x".to_string()));
    assert_eq!(a.counter("c"), 5);
    assert_eq!(a.counter("c"), b.counter("c"));
    assert_eq!(a.get_register::<String>("a").unwrap(), b.get_register::<String>("a").unwrap());
}
