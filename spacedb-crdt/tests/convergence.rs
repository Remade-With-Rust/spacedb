//! The heart of M2: convergence. Deterministic scenarios for each merge
//! behaviour, plus a fuzzed proof that the same set of updates applied in *any*
//! order converges to the same state.

use proptest::prelude::*;
use spacedb_crdt::CrdtDoc;

// ─── deterministic scenarios ─────────────────────────────────────────────────

#[test]
fn register_set_and_get_round_trips() {
    let d = CrdtDoc::new(1);
    d.set_register("title", &"hello".to_string()).unwrap();
    assert_eq!(d.get_register::<String>("title").unwrap(), Some("hello".to_string()));
    assert_eq!(d.get_register::<String>("missing").unwrap(), None);
}

#[test]
fn counter_increments_accumulate_for_one_actor() {
    let d = CrdtDoc::new(1);
    d.increment("views", 3);
    d.increment("views", 4);
    d.increment("views", -1);
    assert_eq!(d.counter("views"), 6);
}

#[test]
fn concurrent_counter_increments_merge_by_sum() {
    let a = CrdtDoc::new(10);
    let b = CrdtDoc::new(20);
    a.increment("likes", 3);
    b.increment("likes", 5);
    // exchange updates (full state is fine here)
    b.apply_update(&a.encode_full()).unwrap();
    a.apply_update(&b.encode_full()).unwrap();
    assert_eq!(a.counter("likes"), 8, "PN-counter merges by summation");
    assert_eq!(b.counter("likes"), 8);
}

#[test]
fn concurrent_register_writes_resolve_deterministically() {
    let a = CrdtDoc::new(10);
    let b = CrdtDoc::new(20);
    // concurrent writes to the same register
    a.set_register("status", &"from-a".to_string()).unwrap();
    b.set_register("status", &"from-b".to_string()).unwrap();
    // exchange
    b.apply_update(&a.encode_full()).unwrap();
    a.apply_update(&b.encode_full()).unwrap();
    let resolved_a = a.get_register::<String>("status").unwrap();
    let resolved_b = b.get_register::<String>("status").unwrap();
    assert_eq!(resolved_a, resolved_b, "both replicas pick the same LWW winner");
    assert!(resolved_a.is_some());
}

#[test]
fn incremental_update_via_state_vector_brings_a_peer_up_to_date() {
    let a = CrdtDoc::new(10);
    let b = CrdtDoc::new(20);
    a.set_register("k", &1u64).unwrap();
    // b learns a's full state once
    b.apply_update(&a.encode_full()).unwrap();
    // a makes a further change; ship only the delta b is missing
    a.set_register("k", &2u64).unwrap();
    let delta = a.encode_update_since(&b.state_vector()).unwrap();
    b.apply_update(&delta).unwrap();
    assert_eq!(b.get_register::<u64>("k").unwrap(), Some(2));
}

// ─── fuzzed convergence: any order → same state ──────────────────────────────

#[derive(Clone, Debug)]
enum Op {
    SetRegister { actor: usize, field: u8, value: i64 },
    Increment { actor: usize, field: u8, delta: i64 },
    TextPush { actor: usize, field: u8, content: String },
    SetAdd { actor: usize, field: u8, elem: u8 },
    SetRemove { actor: usize, field: u8, elem: u8 },
}

const ACTORS: [u64; 3] = [10, 20, 30];
const REG_FIELDS: u8 = 3;
const CNT_FIELDS: u8 = 3;
const TEXT_FIELDS: u8 = 2;
const SET_FIELDS: u8 = 2;
const SET_ELEMS: u8 = 4;

fn elem_name(e: u8) -> String {
    format!("e{e}")
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0..ACTORS.len(), 0..REG_FIELDS, any::<i64>())
            .prop_map(|(actor, field, value)| Op::SetRegister { actor, field, value }),
        (0..ACTORS.len(), 0..CNT_FIELDS, -1000i64..1000)
            .prop_map(|(actor, field, delta)| Op::Increment { actor, field, delta }),
        (0..ACTORS.len(), 0..TEXT_FIELDS, "[a-c]{0,3}")
            .prop_map(|(actor, field, content)| Op::TextPush { actor, field, content }),
        (0..ACTORS.len(), 0..SET_FIELDS, 0..SET_ELEMS)
            .prop_map(|(actor, field, elem)| Op::SetAdd { actor, field, elem }),
        (0..ACTORS.len(), 0..SET_FIELDS, 0..SET_ELEMS)
            .prop_map(|(actor, field, elem)| Op::SetRemove { actor, field, elem }),
    ]
}

fn apply_op(doc: &CrdtDoc, op: &Op) {
    match op {
        Op::SetRegister { field, value, .. } => {
            doc.set_register(&format!("r{field}"), value).unwrap();
        }
        Op::Increment { field, delta, .. } => {
            doc.increment(&format!("c{field}"), *delta);
        }
        Op::TextPush { field, content, .. } => {
            doc.text_push(&format!("t{field}"), content);
        }
        Op::SetAdd { field, elem, .. } => {
            doc.set_add(&format!("s{field}"), &elem_name(*elem));
        }
        Op::SetRemove { field, elem, .. } => {
            doc.set_remove(&format!("s{field}"), &elem_name(*elem));
        }
    }
}

/// Materialize the full observable state of a doc as a comparable value.
fn materialize(doc: &CrdtDoc) -> (Vec<Option<i64>>, Vec<i64>, Vec<String>, Vec<Vec<String>>) {
    let regs = (0..REG_FIELDS)
        .map(|f| doc.get_register::<i64>(&format!("r{f}")).unwrap())
        .collect();
    let cnts = (0..CNT_FIELDS).map(|f| doc.counter(&format!("c{f}"))).collect();
    let texts = (0..TEXT_FIELDS).map(|f| doc.text(&format!("t{f}"))).collect();
    let sets = (0..SET_FIELDS)
        .map(|f| doc.set_members(&format!("s{f}")))
        .collect();
    (regs, cnts, texts, sets)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// Each actor authors its ops on its own replica; the resulting updates are
    /// then applied to two fresh replicas in opposite orders. Both must reach an
    /// identical materialized state.
    ///
    /// (We compare *materialized* state, not raw update bytes: yrs converges the
    /// logical state but its v1 update encoding is not a canonical form — the same
    /// blocks may serialize in apply order — so byte-equality of `encode_full` is
    /// the wrong invariant. `materialize` reads every field this test can touch,
    /// so it fully captures convergence for this domain.)
    #[test]
    fn same_updates_any_order_converge(ops in prop::collection::vec(op_strategy(), 0..40)) {
        // 1. Author the ops on per-actor replicas.
        let authors: Vec<CrdtDoc> = ACTORS.iter().map(|a| CrdtDoc::new(*a)).collect();
        for op in &ops {
            let actor = match op {
                Op::SetRegister { actor, .. }
                | Op::Increment { actor, .. }
                | Op::TextPush { actor, .. }
                | Op::SetAdd { actor, .. }
                | Op::SetRemove { actor, .. } => *actor,
            };
            apply_op(&authors[actor], op);
        }
        let updates: Vec<Vec<u8>> = authors.iter().map(|d| d.encode_full()).collect();

        // 2. Two fresh replicas apply the same updates in opposite orders.
        let r1 = CrdtDoc::new(98);
        for u in &updates {
            r1.apply_update(u).unwrap();
        }
        let r2 = CrdtDoc::new(99);
        for u in updates.iter().rev() {
            r2.apply_update(u).unwrap();
        }

        // 3. They converge to the same observable state regardless of apply order.
        prop_assert_eq!(materialize(&r1), materialize(&r2), "materialized state must converge");
    }
}
