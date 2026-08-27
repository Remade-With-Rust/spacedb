# spacedb-sim

[![crates.io](https://img.shields.io/crates/v/spacedb-sim?logo=rust)](https://crates.io/crates/spacedb-sim)
[![docs.rs](https://img.shields.io/docsrs/spacedb-sim?logo=docsdotrs)](https://docs.rs/spacedb-sim)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-MIT)
[![Remade With Rust](https://img.shields.io/badge/Remade%20With-Rust-000?logo=rust&logoColor=fff)](https://github.com/Remade-With-Rust)
[![By Mata Network](https://img.shields.io/badge/by-Mata%20Network-5b2be0)](https://www.mata.network/)

**A deterministic digital twin of the database.**

A discrete-event simulator that runs a population of **real** `CrdtDoc` replicas
gossiping the **real** anti-entropy protocol over a modeled network — latency,
jitter, packet loss, partitions, churn — all driven by one seeded RNG. The same
scenario and seed produce a byte-identical report, so the simulator is a
*reproducible instrument*: ask "do 500 replicas still converge under 20% loss
and a mid-run partition?", get the same answer every time, and bisect a
regression against it.

Part of [SpaceDB](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/README.md). Dual-licensed **MIT OR Apache-2.0**.

```toml
[dev-dependencies]
spacedb-sim = "0.5"
```

## Convergence under a hostile network

```rust
use spacedb_sim::{NetworkModel, Scenario, Simulation};

let mut sc = Scenario::new(7, 50);                    // seed 7, 50 replicas
sc.network = NetworkModel::new(20, 10, 0.1);          // 20±10 tick latency, 10% loss
let report = Simulation::new(sc).run();

assert!(report.converged);
println!("converged at tick {:?} after {} sync rounds, {} msgs dropped",
         report.converged_at, report.sync_rounds, report.messages_dropped);
```

`Scenario` also carries `partition: Option<PartitionSpec>` and
`offline: Vec<OfflineSpec>` so you can split the network mid-run and take
individual replicas down. `SimReport` is honest about the outcome: `converged`,
`converged_at`, `worst_lag` (how far the most-behind replica still is),
`messages_sent` / `dropped` / `delivered`, and `final_value`.

## The other three sims

| Sim | Scenario | Reports |
|---|---|---|
| `ChurnSim` | homes failing and rejoining across failure domains, with erasure-coded shards, a repair loop and a reclaim loop | `ChurnReport` — did durability hold through the churn? |
| `StrongSim` | quorum members partitioning and healing | `StrongReport` — did the strong tier ever commit divergently? (it must not) |
| `CausalSim` | sessions reading across lagging replicas | `CausalReport` — were read-your-writes / monotonic reads ever violated? |

## Feeding the console

```rust
use spacedb_sim::{observations, churn_observations};
use spacedb_console::{Config, Dashboard};
use spacedb_sim::{Scenario, Simulation};
let mut sim = Simulation::new(Scenario::new(1, 5));
let _report = sim.run();
let now = 1_700_000_000;

let dash = Dashboard::assemble(&observations(&sim), &Config::at(now));
```

The simulator emits the same observation DTOs the real fleet does, so
[`spacedb-console`](https://github.com/Remade-With-Rust/spacedb/tree/HEAD/spacedb-console) renders a simulated fleet exactly as it
renders a live one.

## Determinism

`Rng` is a seeded PRNG, `Scheduler` is a deterministic event queue, and nothing
in the crate reads a wall clock or OS entropy. Same seed → identical run, on any
machine. That is the whole value: a failure you can replay.

## Scope

This is a twin of the **database** — open-core, no maestro / Iron Bank. The full
economic twin is a separate, proprietary system.

## Testing

The workspace defaults to `wasm32`; this crate is native. Test on your host
triple:

```bash
cargo test -p spacedb-sim --target aarch64-apple-darwin   # or your host triple
```

Suites: `sim.rs` (convergence, determinism, partition), `churn.rs` (durability
through failure + repair), `stress.rs` (large populations, heavy loss).

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-MIT) and
[LICENSE-APACHE](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-APACHE).
