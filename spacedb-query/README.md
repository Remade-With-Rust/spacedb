# spacedb-query

[![crates.io](https://img.shields.io/crates/v/spacedb-query?logo=rust)](https://crates.io/crates/spacedb-query)
[![docs.rs](https://img.shields.io/docsrs/spacedb-query?logo=docsdotrs)](https://docs.rs/spacedb-query)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-MIT)
[![Remade With Rust](https://img.shields.io/badge/Remade%20With-Rust-000?logo=rust&logoColor=fff)](https://github.com/Remade-With-Rust)
[![By Mata Network](https://img.shields.io/badge/by-Mata%20Network-5b2be0)](https://www.mata.network/)

**SpaceDB Layer 4 — compute-to-data.**

The environment forbids hauling data to a central brain, so SpaceDB inverts it:
**the query travels to the data; only the answer travels back.** A query is a
deterministic, fuel- and memory-bounded WASM function that runs on the node
holding the data — and because the runtime is deterministic, the result is
**corroboratable**: a host that returns a different answer than its peers is
caught.

Part of [SpaceDB](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/README.md). Dual-licensed **MIT OR Apache-2.0**.

```toml
[dependencies]
spacedb-query = "0.5"
```

## Run a function next to the data

```rust,no_run
use spacedb_query::{FunctionRuntime, RunLimits};
let module_wasm: Vec<u8> = Vec::new();   // a real deterministic WASM module
let input_bytes: Vec<u8> = Vec::new();

let runtime = FunctionRuntime::new().unwrap();
let limits = RunLimits { max_fuel: 100_000_000, max_mem_mb: 64 };

let exec = runtime.run(&module_wasm, &input_bytes, &limits).unwrap();
println!("{:?} fuel={}", exec.output, exec.run.fuel_used);
```

The runtime is `wasmtime` with **no WASI**: no clock, no filesystem, no network,
no randomness. A function that cannot observe anything nondeterministic cannot
produce a nondeterministic answer — which is what makes the next section work.
Fuel exhaustion and the memory ceiling are hard stops, so an untrusted function
cannot pin a home's CPU or RAM.

## Corroboration

Every run yields a `FunctionRun` attestation: `workload_hash`, `input_digest`,
`output_digest`, `fuel_used`, `mem_peak_mb`.

```rust
use spacedb_query::{corroborate, Corroboration, FunctionRun};

// Two independent hosts ran the same (workload, input) — so every field must match.
let run_a = FunctionRun {
    workload_hash: [1u8; 32],
    input_digest: [2u8; 32],
    output_digest: [3u8; 32],
    fuel_used: 12_345,
    mem_peak_mb: 1,
};
let run_b = run_a.clone();

assert_eq!(corroborate(&run_a, &run_b), Corroboration::Agree);
```

Honest runs of the same `(workload, input)` agree on **every** field. A wrong
output, or a padded fuel count billed for work not done, is `Disagree`. Fanning
that comparison across independent marketplace hosts is the MATA seam; the
runtime, the attestation, and the comparison are here.

## Pinned snapshots and rights

`Snapshot::pin(bytes, frontier)` freezes exactly what a query reads, and its
`hash()` goes into the attestation — so two hosts corroborating a result are
provably comparing the same input state, not two different moments in time.

`FunctionCtx` carries that snapshot plus `CtxRights { read, write }` and an
explicit **denied** set: collections whose ciphertext the host holds but
provably cannot compute on. Serving those to server-side code would break the
zero-knowledge promise, so the context refuses them. Accumulated writes are
exposed as a `writes_digest` rather than applied behind the caller's back.

## Partition-aware map-reduce

```rust,ignore
use spacedb_query::{run_query, FunctionRuntime, QueryPlan, RunLimits, Shard, Snapshot};

let runtime = FunctionRuntime::new().unwrap();
let plan = QueryPlan {
    map_wasm: &map_wasm,
    reduce_wasm: &reduce_wasm,
    limits: RunLimits::default(),
};
let shards = vec![
    Shard::new("home-1", Snapshot::pin(snapshot_a, frontier_a)),
    Shard::new("home-2", Snapshot::pin(snapshot_b, frontier_b)).unreachable(),
];

let outcome = run_query(&runtime, &plan, &shards).unwrap();
if !outcome.coverage.is_complete() {
    // honest: N shards were unreachable — this is a partial answer
    eprintln!("{} shard(s) missing", outcome.coverage.missing());
}
```

The planner maps over reachable shards and reduces the partials, and reports
`Coverage` honestly. A partial answer is labeled partial; it is never rounded up
to complete.

## Open-core boundary

Built on `wasmtime` directly — the same engine MATA's `maestro-fn-runtime`
wraps — with no MATA dependency.

## Testing

The workspace defaults to `wasm32`; this crate is native. Test on your host
triple:

```bash
cargo test -p spacedb-query --target aarch64-apple-darwin   # or your host triple
```

Suites: `runtime.rs` (determinism, fuel/memory limits, module validation),
`functions.rs` (context, rights, denied collections), `planner.rs` (map-reduce,
coverage under partition).

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-MIT) and
[LICENSE-APACHE](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-APACHE).
