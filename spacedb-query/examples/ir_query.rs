//! Deterministic instruction-count driver for the WASM query path (callgrind Ir):
//! a map-reduce over shards (`run_query`) and a host-call-heavy database
//! function (`run_with_ctx`). Output digests and fuel are the work-parity
//! anchors — fuel in particular proves the guest did identical work.

use std::collections::BTreeSet;

use spacedb_query::{
    run_query, CtxRights, FunctionCtx, FunctionRuntime, QueryPlan, RecordSnapshot, RunLimits,
    Shard, Snapshot,
};

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

/// 300 `get`s, one `put`, one `query` over collection `c`; returns the last get.
const HOST_LOOP_WAT: &str = r#"
    (module
      (import "spacedb" "host_call" (func $host (param i32 i32 i32 i32) (result i64)))
      (memory (export "memory") 1)
      (data (i32.const 0) "get")
      (data (i32.const 8) "c\00k0007")
      (data (i32.const 32) "query")
      (data (i32.const 40) "c")
      (data (i32.const 48) "put")
      (data (i32.const 56) "c\00new\00value-bytes")
      (global $bump (mut i32) (i32.const 1024))
      (func (export "alloc") (param $len i32) (result i32)
        (local $p i32)
        (local.set $p (global.get $bump))
        (global.set $bump (i32.add (global.get $bump) (local.get $len)))
        (local.get $p))
      (func (export "run") (param $in_ptr i32) (param $in_len i32) (result i64)
        (local $i i32) (local $last i64)
        (block $done
          (loop $loop
            (br_if $done (i32.ge_u (local.get $i) (i32.const 300)))
            (local.set $last (call $host (i32.const 0) (i32.const 3) (i32.const 8) (i32.const 7)))
            (local.set $i (i32.add (local.get $i) (i32.const 1)))
            (br $loop)))
        (drop (call $host (i32.const 48) (i32.const 3) (i32.const 56) (i32.const 17)))
        (drop (call $host (i32.const 32) (i32.const 5) (i32.const 40) (i32.const 1)))
        (local.get $last)))
"#;

fn main() {
    let rt = FunctionRuntime::new().unwrap();
    let map = wat::parse_str(SUM_MAP_WAT).unwrap();
    let reduce = wat::parse_str(SUM_REDUCE_WAT).unwrap();
    let host = wat::parse_str(HOST_LOOP_WAT).unwrap();

    let mut check = 0u64;
    let mut fuel = 0u64;
    for rep in 0..3 {
        let shards: Vec<Shard> = (0..8)
            .map(|i| {
                let data: Vec<u8> = (0..2048)
                    .map(|b| ((b * 7 + i * 13 + rep) % 251) as u8)
                    .collect();
                Shard::new(format!("s{i}"), Snapshot::pin(data, vec![]))
            })
            .collect();
        let plan = QueryPlan {
            map_wasm: &map,
            reduce_wasm: &reduce,
            limits: RunLimits::default(),
        };
        let out = run_query(&rt, &plan, &shards).unwrap();
        let value = out.output.unwrap();
        check = check
            .wrapping_mul(31)
            .wrapping_add(value.iter().map(|&b| b as u64).sum::<u64>());
        fuel += out.map_runs.iter().map(|r| r.fuel_used).sum::<u64>();
    }

    let mut snap = RecordSnapshot::new();
    for coll in ["b", "c", "d"] {
        for i in 0..200 {
            snap.insert(
                (coll.to_string(), format!("k{i:04}")),
                vec![(i % 251) as u8; 32],
            );
        }
    }
    let rights = CtxRights {
        read: true,
        write: true,
    };
    for _ in 0..4 {
        let ctx = FunctionCtx::new(snap.clone(), rights, BTreeSet::new());
        let o = rt
            .run_with_ctx(&host, b"in", &RunLimits::default(), ctx)
            .unwrap();
        fuel += o.execution.run.fuel_used;
        check ^= u64::from_le_bytes(o.execution.run.output_digest[..8].try_into().unwrap());
        check ^= u64::from_le_bytes(o.ctx.writes_digest()[..8].try_into().unwrap());
    }
    println!("fuel {fuel} check {check:016x}");
}
