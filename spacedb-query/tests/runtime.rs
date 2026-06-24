//! M6-S1: deterministic, sandboxed compute-to-data with corroboratable results.

use spacedb_query::{corroborate, Corroboration, FunctionRuntime, QueryError, RunLimits};

/// A minimal ABI module: a bump allocator + a `run` that echoes its input.
const ECHO_WAT: &str = r#"
    (module
      (memory (export "memory") 1)
      (global $bump (mut i32) (i32.const 1024))
      (func $alloc (export "alloc") (param $len i32) (result i32)
        (local $p i32)
        (local.set $p (global.get $bump))
        (global.set $bump (i32.add (global.get $bump) (local.get $len)))
        (local.get $p))
      (func (export "run") (param $in_ptr i32) (param $in_len i32) (result i64)
        (i64.or
          (i64.shl (i64.extend_i32_u (local.get $in_ptr)) (i64.const 32))
          (i64.extend_i32_u (local.get $in_len)))))
"#;

/// A real transform: sums the input bytes and returns the 4-byte little-endian
/// total. Proves the runtime computes (not just echoes) and that the computation
/// corroborates.
const SUM_WAT: &str = r#"
    (module
      (memory (export "memory") 1)
      (global $bump (mut i32) (i32.const 1024))
      (func $alloc (export "alloc") (param $len i32) (result i32)
        (local $p i32)
        (local.set $p (global.get $bump))
        (global.set $bump (i32.add (global.get $bump) (local.get $len)))
        (local.get $p))
      (func (export "run") (param $in_ptr i32) (param $in_len i32) (result i64)
        (local $i i32)
        (local $sum i32)
        (local $out i32)
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

fn wasm(wat: &str) -> Vec<u8> {
    wat::parse_str(wat).unwrap()
}

#[test]
fn runs_a_function_and_attests_it() {
    let rt = FunctionRuntime::new();
    let module = wasm(ECHO_WAT);
    let exec = rt.run(&module, b"hello", &RunLimits::default()).unwrap();

    assert_eq!(exec.output, b"hello"); // echo
    assert_eq!(exec.run.workload_hash, *blake3::hash(&module).as_bytes());
    assert_eq!(exec.run.input_digest, *blake3::hash(b"hello").as_bytes());
    assert_eq!(exec.run.output_digest, *blake3::hash(b"hello").as_bytes());
    assert!(exec.run.fuel_used > 0);
    assert!(exec.run.fuel_used < RunLimits::default().max_fuel);
}

#[test]
fn computes_a_real_transform_on_node() {
    let rt = FunctionRuntime::new();
    let module = wasm(SUM_WAT);
    // bytes 1 + 2 + 3 + 250 = 256 -> little-endian 4-byte [0, 1, 0, 0]
    let exec = rt.run(&module, &[1, 2, 3, 250], &RunLimits::default()).unwrap();
    assert_eq!(exec.output, vec![0, 1, 0, 0]);
}

#[test]
fn identical_runs_corroborate() {
    let rt = FunctionRuntime::new();
    let module = wasm(SUM_WAT);
    let a = rt.run(&module, b"lead-capture-payload", &RunLimits::default()).unwrap();
    let b = rt.run(&module, b"lead-capture-payload", &RunLimits::default()).unwrap();

    // every deterministic field matches — the property corroboration relies on
    assert_eq!(a.run, b.run);
    assert_eq!(corroborate(&a.run, &b.run), Corroboration::Agree);
}

#[test]
fn a_divergent_result_is_caught() {
    let rt = FunctionRuntime::new();
    let module = wasm(SUM_WAT);
    let honest = rt.run(&module, b"payload", &RunLimits::default()).unwrap();

    // a lying host claims a different output for the same workload + input
    let mut liar = honest.run.clone();
    liar.output_digest[0] ^= 0x01;
    assert_eq!(corroborate(&honest.run, &liar), Corroboration::Disagree);
}

#[test]
fn fuel_exhaustion_traps_rather_than_hanging() {
    let rt = FunctionRuntime::new();
    let module = wasm(SUM_WAT);
    let err = rt
        .run(&module, b"hello", &RunLimits { max_fuel: 1, max_mem_mb: 64 })
        .unwrap_err();
    assert!(matches!(err, QueryError::Trap(_)), "expected a fuel trap, got {err:?}");
}

#[test]
fn a_module_missing_the_abi_is_rejected() {
    let rt = FunctionRuntime::new();
    let module = wat::parse_str("(module (memory (export \"memory\") 1))").unwrap();
    let err = rt.run(&module, b"x", &RunLimits::default()).unwrap_err();
    assert!(matches!(err, QueryError::MissingExport("alloc")), "got {err:?}");
}
