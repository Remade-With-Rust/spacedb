//! M6-S2: pinned-snapshot reads + the partition-aware map-reduce planner.

use spacedb_crdt::CrdtDoc;
use spacedb_query::{run_query, FunctionRuntime, QueryPlan, RunLimits, Shard, Snapshot};

/// map: sum the snapshot's bytes, returning the 4-byte little-endian total.
const SUM_MAP_WAT: &str = r#"
    (module
      (memory (export "memory") 1)
      (global $bump (mut i32) (i32.const 1024))
      (func $alloc (export "alloc") (param $len i32) (result i32)
        (local $p i32)
        (local.set $p (global.get $bump))
        (global.set $bump (i32.add (global.get $bump) (local.get $len)))
        (local.get $p))
      (func (export "run") (param $in_ptr i32) (param $in_len i32) (result i64)
        (local $i i32) (local $sum i32) (local $out i32)
        (local.set $i (i32.const 0))
        (local.set $sum (i32.const 0))
        (block $done
          (loop $loop
            (br_if $done (i32.ge_u (local.get $i) (local.get $in_len)))
            (local.set $sum
              (i32.add (local.get $sum)
                (i32.load8_u (i32.add (local.get $in_ptr) (local.get $i)))))
            (local.set $i (i32.add (local.get $i) (i32.const 1)))
            (br $loop)))
        (local.set $out (call $alloc (i32.const 4)))
        (i32.store (local.get $out) (local.get $sum))
        (i64.or
          (i64.shl (i64.extend_i32_u (local.get $out)) (i64.const 32))
          (i64.extend_i32_u (i32.const 4)))))
"#;

/// reduce: input is `len(a) ‖ a ‖ b` where a and b are 4-byte i32 partials; add
/// them. Associative and commutative (integer addition).
const SUM_REDUCE_WAT: &str = r#"
    (module
      (memory (export "memory") 1)
      (global $bump (mut i32) (i32.const 1024))
      (func $alloc (export "alloc") (param $len i32) (result i32)
        (local $p i32)
        (local.set $p (global.get $bump))
        (global.set $bump (i32.add (global.get $bump) (local.get $len)))
        (local.get $p))
      (func (export "run") (param $in_ptr i32) (param $in_len i32) (result i64)
        (local $a_len i32) (local $a i32) (local $b i32) (local $out i32)
        (local.set $a_len (i32.load (local.get $in_ptr)))
        (local.set $a (i32.load (i32.add (local.get $in_ptr) (i32.const 4))))
        (local.set $b (i32.load (i32.add (i32.add (local.get $in_ptr) (i32.const 4)) (local.get $a_len))))
        (local.set $out (call $alloc (i32.const 4)))
        (i32.store (local.get $out) (i32.add (local.get $a) (local.get $b)))
        (i64.or
          (i64.shl (i64.extend_i32_u (local.get $out)) (i64.const 32))
          (i64.extend_i32_u (i32.const 4)))))
"#;

fn wasm(wat: &str) -> Vec<u8> {
    wat::parse_str(wat).unwrap()
}

fn shard(id: &str, data: &[u8]) -> Shard {
    Shard::new(id, Snapshot::pin(data.to_vec(), vec![]))
}

fn i32_le(bytes: &[u8]) -> i32 {
    i32::from_le_bytes(bytes.try_into().unwrap())
}

fn plan<'a>(map: &'a [u8], reduce: &'a [u8]) -> QueryPlan<'a> {
    QueryPlan {
        map_wasm: map,
        reduce_wasm: reduce,
        limits: RunLimits::default(),
    }
}

#[test]
fn map_reduces_across_shards_with_full_coverage() {
    let rt = FunctionRuntime::new();
    let map = wasm(SUM_MAP_WAT);
    let reduce = wasm(SUM_REDUCE_WAT);
    let shards = vec![
        shard("a", &[1, 2, 3]),   // 6
        shard("b", &[10, 20]),    // 30
        shard("c", &[100, 4]),    // 104
    ];

    let outcome = run_query(&rt, &plan(&map, &reduce), &shards).unwrap();
    assert!(outcome.coverage.is_complete());
    assert_eq!(outcome.coverage.shards_computed, 3);
    assert_eq!(i32_le(outcome.output.as_ref().unwrap()), 140); // 6 + 30 + 104
    assert_eq!(outcome.map_runs.len(), 3); // one attestation per shard
}

#[test]
fn a_partition_yields_a_partial_result_flagged_with_coverage() {
    let rt = FunctionRuntime::new();
    let map = wasm(SUM_MAP_WAT);
    let reduce = wasm(SUM_REDUCE_WAT);
    let shards = vec![
        shard("a", &[1, 2, 3]),         // 6
        shard("b", &[10, 20]).unreachable(), // host down — excluded
        shard("c", &[100, 4]),          // 104
    ];

    let outcome = run_query(&rt, &plan(&map, &reduce), &shards).unwrap();
    assert!(!outcome.coverage.is_complete(), "result must be flagged partial");
    assert_eq!(outcome.coverage.shards_computed, 2);
    assert_eq!(outcome.coverage.missing(), 1);
    assert_eq!(i32_le(outcome.output.as_ref().unwrap()), 110); // 6 + 104, NOT 140
}

#[test]
fn the_reduce_is_order_independent() {
    let rt = FunctionRuntime::new();
    let map = wasm(SUM_MAP_WAT);
    let reduce = wasm(SUM_REDUCE_WAT);
    let forward = vec![shard("a", &[1, 2, 3]), shard("b", &[10, 20]), shard("c", &[100, 4])];
    let reversed = vec![shard("c", &[100, 4]), shard("b", &[10, 20]), shard("a", &[1, 2, 3])];

    let a = run_query(&rt, &plan(&map, &reduce), &forward).unwrap();
    let b = run_query(&rt, &plan(&map, &reduce), &reversed).unwrap();
    assert_eq!(a.output, b.output, "a commutative reduce is partition-order independent");
}

#[test]
fn no_reachable_shards_yields_no_output() {
    let rt = FunctionRuntime::new();
    let map = wasm(SUM_MAP_WAT);
    let reduce = wasm(SUM_REDUCE_WAT);
    let shards = vec![
        shard("a", &[1]).unreachable(),
        shard("b", &[2]).unreachable(),
    ];

    let outcome = run_query(&rt, &plan(&map, &reduce), &shards).unwrap();
    assert!(outcome.output.is_none());
    assert_eq!(outcome.coverage.shards_computed, 0);
    assert!(!outcome.coverage.is_complete());
}

#[test]
fn pins_a_consistent_snapshot_of_a_convergent_document() {
    let rt = FunctionRuntime::new();
    let map = wasm(SUM_MAP_WAT);
    let reduce = wasm(SUM_REDUCE_WAT);

    let doc = CrdtDoc::new(1);
    doc.set_register("title", &"hello".to_string()).unwrap();
    doc.increment("views", 3);

    // pin a CRDT-native snapshot: the encoded state at the current frontier
    let pinned = Snapshot::pin(doc.encode_full(), doc.state_vector());
    assert_eq!(pinned.frontier(), doc.state_vector());

    // re-pinning the same state is deterministic (same content hash)
    let again = Snapshot::pin(doc.encode_full(), doc.state_vector());
    assert_eq!(pinned.hash(), again.hash());

    // a query runs over the pinned snapshot
    let shards = vec![Shard::new("doc", pinned.clone())];
    let outcome = run_query(&rt, &plan(&map, &reduce), &shards).unwrap();
    assert!(outcome.output.is_some());
    assert!(outcome.coverage.is_complete());

    // mutating the doc after pinning does not change the pinned view — the query
    // read a consistent frontier, not live state
    doc.increment("views", 100);
    let newer = Snapshot::pin(doc.encode_full(), doc.state_vector());
    assert_ne!(pinned.hash(), newer.hash(), "the new state is a different snapshot");
}
