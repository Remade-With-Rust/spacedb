//! Cross-engine equivalence suite.
//!
//! Every scenario runs against **both** engines — the in-memory one and the
//! redb-backed one — through one shared body. The point of having two engines is
//! that they must be observationally identical: the in-memory engine is only a
//! faithful test double if it reproduces redb's transaction semantics (atomic
//! commit, rollback-on-drop, read-your-writes, logical-order ranges). Running the
//! same assertions over both is what proves that.

use spacedb_store::{Durability, KvEngine, MemEngine, RedbEngine, Table, WriteTx};

// ─── scenarios (each generic over the engine) ────────────────────────────────

fn put_get_round_trip(e: &impl KvEngine) {
    let t: Table<u64, String> = Table::new("items");
    let mut w = e.begin_write(Durability::Immediate).unwrap();
    t.put(&mut w, &42, &"hello".to_string()).unwrap();
    w.commit().unwrap();

    let r = e.begin_read().unwrap();
    assert_eq!(t.get(&r, &42).unwrap(), Some("hello".to_string()));
    assert_eq!(t.get(&r, &7).unwrap(), None);
}

fn range_is_ordered(e: &impl KvEngine) {
    let t: Table<u64, String> = Table::new("ordered");
    let mut w = e.begin_write(Durability::Immediate).unwrap();
    for k in [5u64, 1, 3, 2, 4] {
        t.put(&mut w, &k, &format!("v{k}")).unwrap();
    }
    w.commit().unwrap();

    let r = e.begin_read().unwrap();
    let keys: Vec<u64> = t.range(&r, &0, &100).unwrap().into_iter().map(|(k, _)| k).collect();
    assert_eq!(keys, vec![1, 2, 3, 4, 5], "range must return logical key order");
}

fn composite_key_ordering(e: &impl KvEngine) {
    // Proves the order-preserving composite key encoding survives a real
    // engine round-trip: a longer first component must not bleed into the second.
    let t: Table<(u64, String), u64> = Table::new("composite");
    let mut w = e.begin_write(Durability::Immediate).unwrap();
    t.put(&mut w, &(2, "a".to_string()), &0).unwrap();
    t.put(&mut w, &(1, "z".to_string()), &0).unwrap();
    t.put(&mut w, &(2, "b".to_string()), &0).unwrap();
    w.commit().unwrap();

    let r = e.begin_read().unwrap();
    let keys: Vec<(u64, String)> = t
        .range(&r, &(0, String::new()), &(100, String::new()))
        .unwrap()
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(
        keys,
        vec![(1, "z".to_string()), (2, "a".to_string()), (2, "b".to_string())]
    );
}

fn range_is_half_open(e: &impl KvEngine) {
    let t: Table<u64, u64> = Table::new("halfopen");
    let mut w = e.begin_write(Durability::Immediate).unwrap();
    for k in 1..=5u64 {
        t.put(&mut w, &k, &k).unwrap();
    }
    w.commit().unwrap();

    let r = e.begin_read().unwrap();
    let keys: Vec<u64> = t.range(&r, &2, &4).unwrap().into_iter().map(|(k, _)| k).collect();
    assert_eq!(keys, vec![2, 3], "[lo, hi) excludes hi");
}

fn delete_and_overwrite(e: &impl KvEngine) {
    let t: Table<u64, String> = Table::new("del");
    let mut w = e.begin_write(Durability::Immediate).unwrap();
    t.put(&mut w, &1, &"a".to_string()).unwrap();
    t.put(&mut w, &1, &"b".to_string()).unwrap(); // overwrite
    w.commit().unwrap();

    let r = e.begin_read().unwrap();
    assert_eq!(t.get(&r, &1).unwrap(), Some("b".to_string()));
    drop(r); // release the read snapshot before opening a writer (single-writer engines)

    let mut w2 = e.begin_write(Durability::Immediate).unwrap();
    assert!(t.delete(&mut w2, &1).unwrap(), "delete reports prior presence");
    assert!(!t.delete(&mut w2, &1).unwrap(), "second delete is a no-op (read-your-writes)");
    w2.commit().unwrap();

    let r2 = e.begin_read().unwrap();
    assert_eq!(t.get(&r2, &1).unwrap(), None);
}

fn multi_table_atomic_commit(e: &impl KvEngine) {
    let a: Table<u64, String> = Table::new("table_a");
    let b: Table<u64, String> = Table::new("table_b");

    let mut w = e.begin_write(Durability::Immediate).unwrap();
    a.put(&mut w, &1, &"x".to_string()).unwrap();
    b.put(&mut w, &2, &"y".to_string()).unwrap();
    w.commit().unwrap();

    let r = e.begin_read().unwrap();
    assert_eq!(a.get(&r, &1).unwrap(), Some("x".to_string()));
    assert_eq!(b.get(&r, &2).unwrap(), Some("y".to_string()));
}

fn rollback_on_drop(e: &impl KvEngine) {
    let t: Table<u64, String> = Table::new("rollback");

    // Committed baseline.
    {
        let mut w = e.begin_write(Durability::Immediate).unwrap();
        t.put(&mut w, &1, &"keep".to_string()).unwrap();
        w.commit().unwrap();
    }

    // A transaction that mutates then is dropped WITHOUT committing.
    {
        let mut w = e.begin_write(Durability::Immediate).unwrap();
        t.put(&mut w, &2, &"vanish".to_string()).unwrap();
        assert!(t.delete(&mut w, &1).unwrap());
        // read-your-writes inside the txn sees the staged mutations
        assert_eq!(t.get(&w, &2).unwrap(), Some("vanish".to_string()));
        assert_eq!(t.get(&w, &1).unwrap(), None);
        // ...and then we drop `w` here, with no commit.
    }

    let r = e.begin_read().unwrap();
    assert_eq!(t.get(&r, &1).unwrap(), Some("keep".to_string()), "rollback restores prior state");
    assert_eq!(t.get(&r, &2).unwrap(), None, "uncommitted put must not survive");
}

fn read_your_writes_in_range(e: &impl KvEngine) {
    let t: Table<u64, u64> = Table::new("ryw");
    {
        let mut w = e.begin_write(Durability::Immediate).unwrap();
        t.put(&mut w, &10, &100).unwrap();
        w.commit().unwrap();
    }

    let mut w = e.begin_write(Durability::Immediate).unwrap();
    t.put(&mut w, &5, &50).unwrap();
    assert!(t.delete(&mut w, &10).unwrap());
    // A range scan within the same txn reflects the staged put(5) + delete(10).
    let keys: Vec<u64> = t.range(&w, &0, &100).unwrap().into_iter().map(|(k, _)| k).collect();
    assert_eq!(keys, vec![5]);
    w.commit().unwrap();
}

fn eventual_durability_round_trips(e: &impl KvEngine) {
    let t: Table<u64, String> = Table::new("eventual");
    let mut w = e.begin_write(Durability::Eventual).unwrap();
    t.put(&mut w, &1, &"e".to_string()).unwrap();
    w.commit().unwrap();

    let r = e.begin_read().unwrap();
    assert_eq!(t.get(&r, &1).unwrap(), Some("e".to_string()));
}

// ─── run every scenario against both engines ─────────────────────────────────

macro_rules! engine_suite {
    ($modname:ident, $make:expr, [$($scenario:ident),* $(,)?]) => {
        mod $modname {
            use super::*;
            $(
                #[test]
                fn $scenario() {
                    // `_holder` keeps any backing resource (e.g. the redb tempdir)
                    // alive for the duration of the scenario.
                    let (_holder, engine) = $make;
                    super::$scenario(&engine);
                }
            )*
        }
    };
}

engine_suite!(
    mem,
    ((), MemEngine::new()),
    [
        put_get_round_trip,
        range_is_ordered,
        composite_key_ordering,
        range_is_half_open,
        delete_and_overwrite,
        multi_table_atomic_commit,
        rollback_on_drop,
        read_your_writes_in_range,
        eventual_durability_round_trips,
    ]
);

engine_suite!(
    redb,
    {
        let dir = tempfile::tempdir().unwrap();
        let engine = RedbEngine::open(dir.path().join("store.redb")).unwrap();
        (dir, engine)
    },
    [
        put_get_round_trip,
        range_is_ordered,
        composite_key_ordering,
        range_is_half_open,
        delete_and_overwrite,
        multi_table_atomic_commit,
        rollback_on_drop,
        read_your_writes_in_range,
        eventual_durability_round_trips,
    ]
);
