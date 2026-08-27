//! Database-Function runtime tests: the host-call ABI, read-your-writes, the
//! determinism contract over (module, input, snapshot), rights + zero-knowledge
//! enforcement, deploy-time import validation, and all-or-nothing failure.

use std::collections::BTreeSet;

use spacedb_query::{corroborate, Corroboration, CtxRights, FunctionCtx, FunctionRuntime, RecordSnapshot, RunLimits};

const RW: CtxRights = CtxRights { read: true, write: true };

/// A guest that exercises the whole surface with static payloads:
/// `put c/dst = "hello"`, then `get c/dst` (read-your-writes) and return the
/// response bytes (`[1] ++ "hello"`) as its output.
const PUT_GET_WAT: &str = r#"
    (module
      (import "spacedb" "host_call" (func $host (param i32 i32 i32 i32) (result i64)))
      (memory (export "memory") 1)
      (data (i32.const 0) "put")
      (data (i32.const 8) "c\00dst\00hello")
      (data (i32.const 32) "get")
      (data (i32.const 40) "c\00dst")
      (global $bump (mut i32) (i32.const 1024))
      (func (export "alloc") (param $len i32) (result i32)
        (local $p i32)
        (local.set $p (global.get $bump))
        (global.set $bump (i32.add (global.get $bump) (local.get $len)))
        (local.get $p))
      (func (export "run") (param $in_ptr i32) (param $in_len i32) (result i64)
        (drop (call $host (i32.const 0) (i32.const 3) (i32.const 8) (i32.const 11)))
        (call $host (i32.const 32) (i32.const 3) (i32.const 40) (i32.const 5))))
"#;

/// A guest that reads `c/k1` from the snapshot and returns the response.
const GET_WAT: &str = r#"
    (module
      (import "spacedb" "host_call" (func $host (param i32 i32 i32 i32) (result i64)))
      (memory (export "memory") 1)
      (data (i32.const 0) "get")
      (data (i32.const 8) "c\00k1")
      (global $bump (mut i32) (i32.const 1024))
      (func (export "alloc") (param $len i32) (result i32)
        (local $p i32)
        (local.set $p (global.get $bump))
        (global.set $bump (i32.add (global.get $bump) (local.get $len)))
        (local.get $p))
      (func (export "run") (param $in_ptr i32) (param $in_len i32) (result i64)
        (call $host (i32.const 0) (i32.const 3) (i32.const 8) (i32.const 4))))
"#;

/// A guest that puts, then unconditionally traps — its writes must never survive.
const PUT_THEN_TRAP_WAT: &str = r#"
    (module
      (import "spacedb" "host_call" (func $host (param i32 i32 i32 i32) (result i64)))
      (memory (export "memory") 1)
      (data (i32.const 0) "put")
      (data (i32.const 8) "c\00dst\00hello")
      (global $bump (mut i32) (i32.const 1024))
      (func (export "alloc") (param $len i32) (result i32)
        (local $p i32)
        (local.set $p (global.get $bump))
        (global.set $bump (i32.add (global.get $bump) (local.get $len)))
        (local.get $p))
      (func (export "run") (param $in_ptr i32) (param $in_len i32) (result i64)
        (drop (call $host (i32.const 0) (i32.const 3) (i32.const 8) (i32.const 11)))
        unreachable))
"#;

fn snapshot(entries: &[(&str, &str, &[u8])]) -> RecordSnapshot {
    entries
        .iter()
        .map(|(c, id, v)| ((c.to_string(), id.to_string()), v.to_vec()))
        .collect()
}

fn ctx(snap: RecordSnapshot, rights: CtxRights, denied: &[&str]) -> FunctionCtx {
    FunctionCtx::new(snap, rights, denied.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>())
}

#[test]
fn a_function_buffers_writes_and_reads_its_own_writes() {
    let rt = FunctionRuntime::new();
    let wasm = wat::parse_str(PUT_GET_WAT).unwrap();

    let out = rt
        .run_with_ctx(&wasm, b"in", &RunLimits::default(), ctx(snapshot(&[]), RW, &[]))
        .unwrap();

    // Output = the get response: present-tag + the value this run itself put.
    assert_eq!(out.execution.output, b"\x01hello".to_vec());
    // The write-set carries the buffered put — nothing was applied anywhere yet.
    let writes = out.ctx.writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(
        writes.get(&("c".to_string(), "dst".to_string())),
        Some(&Some(b"hello".to_vec()))
    );
    assert!(out.ctx.write_bytes() > 0);
    assert!(out.execution.run.fuel_used > 0);
}

#[test]
fn reads_come_from_the_pinned_snapshot() {
    let rt = FunctionRuntime::new();
    let wasm = wat::parse_str(GET_WAT).unwrap();

    let hit = rt
        .run_with_ctx(
            &wasm,
            b"",
            &RunLimits::default(),
            ctx(snapshot(&[("c", "k1", b"pinned")]), RW, &[]),
        )
        .unwrap();
    assert_eq!(hit.execution.output, b"\x01pinned".to_vec());

    let miss = rt
        .run_with_ctx(&wasm, b"", &RunLimits::default(), ctx(snapshot(&[]), RW, &[]))
        .unwrap();
    assert_eq!(miss.execution.output, vec![0], "absent record reads as the absent tag");
}

#[test]
fn the_determinism_contract_extends_to_mutations() {
    let rt = FunctionRuntime::new();
    let wasm = wat::parse_str(PUT_GET_WAT).unwrap();
    let snap = snapshot(&[("c", "seed", b"x")]);

    let a = rt
        .run_with_ctx(&wasm, b"in", &RunLimits::default(), ctx(snap.clone(), RW, &[]))
        .unwrap();
    let b = rt
        .run_with_ctx(&wasm, b"in", &RunLimits::default(), ctx(snap.clone(), RW, &[]))
        .unwrap();

    // same module + input + snapshot ⇒ same attestation AND same write-set.
    assert_eq!(corroborate(&a.execution.run, &b.execution.run), Corroboration::Agree);
    assert_eq!(a.ctx.writes_digest(), b.ctx.writes_digest());
    assert_eq!(a.execution.run.fuel_used, b.execution.run.fuel_used);

    // A different snapshot must NOT corroborate (input_digest binds snapshot_hash).
    let c = rt
        .run_with_ctx(
            &wasm,
            b"in",
            &RunLimits::default(),
            ctx(snapshot(&[("c", "seed", b"DIFFERENT")]), RW, &[]),
        )
        .unwrap();
    assert_ne!(
        a.execution.run.input_digest, c.execution.run.input_digest,
        "runs over different data must not be corroboratable as the same job"
    );
}

#[test]
fn rights_are_checked_per_host_call() {
    let rt = FunctionRuntime::new();
    let put_wasm = wat::parse_str(PUT_GET_WAT).unwrap();
    let get_wasm = wat::parse_str(GET_WAT).unwrap();

    // Read-only caller invoking a writing function: the put traps the whole run.
    let read_only = CtxRights { read: true, write: false };
    let err = rt
        .run_with_ctx(&put_wasm, b"", &RunLimits::default(), ctx(snapshot(&[]), read_only, &[]))
        .unwrap_err();
    assert!(err.to_string().contains("database:write"), "got: {err}");

    // Write-only caller invoking a reading function: the get traps.
    let write_only = CtxRights { read: false, write: true };
    let err = rt
        .run_with_ctx(&get_wasm, b"", &RunLimits::default(), ctx(snapshot(&[]), write_only, &[]))
        .unwrap_err();
    assert!(err.to_string().contains("database:read"), "got: {err}");
}

#[test]
fn zero_knowledge_collections_are_refused() {
    let rt = FunctionRuntime::new();
    let wasm = wat::parse_str(GET_WAT).unwrap();
    let err = rt
        .run_with_ctx(
            &wasm,
            b"",
            &RunLimits::default(),
            ctx(snapshot(&[("c", "k1", b"ciphertext")]), RW, &["c"]),
        )
        .unwrap_err();
    assert!(err.to_string().contains("zero-knowledge"), "got: {err}");
}

#[test]
fn a_trapped_run_still_surfaces_no_write_set() {
    let rt = FunctionRuntime::new();
    let wasm = wat::parse_str(PUT_THEN_TRAP_WAT).unwrap();
    // The run errors — there is no outcome, hence no write-set to apply. The put it
    // buffered before trapping dies with the run (all-or-nothing by construction).
    let err = rt
        .run_with_ctx(&wasm, b"", &RunLimits::default(), ctx(snapshot(&[]), RW, &[]))
        .unwrap_err();
    assert!(err.to_string().contains("unreachable") || err.to_string().contains("trap"), "got: {err}");
}

#[test]
fn deploy_validation_allows_only_the_host_call_import() {
    let rt = FunctionRuntime::new();

    // The legit function validates.
    rt.validate_function(&wat::parse_str(PUT_GET_WAT).unwrap()).unwrap();

    // A module importing anything else is rejected at deploy.
    let smuggler = wat::parse_str(
        r#"(module
             (import "wasi_snapshot_preview1" "random_get" (func (param i32 i32) (result i32)))
             (memory (export "memory") 1)
             (func (export "alloc") (param i32) (result i32) (i32.const 0))
             (func (export "run") (param i32 i32) (result i64) (i64.const 0)))"#,
    )
    .unwrap();
    let err = rt.validate_function(&smuggler).unwrap_err();
    assert!(err.to_string().contains("forbidden import"), "got: {err}");

    // A module missing the base ABI is rejected too.
    let no_abi = wat::parse_str("(module (memory (export \"memory\") 1))").unwrap();
    let err = rt.validate_function(&no_abi).unwrap_err();
    assert!(err.to_string().contains("missing required export"), "got: {err}");
}
