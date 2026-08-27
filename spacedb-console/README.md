# spacedb-console

[![crates.io](https://img.shields.io/crates/v/spacedb-console?logo=rust)](https://crates.io/crates/spacedb-console)
[![docs.rs](https://img.shields.io/docsrs/spacedb-console?logo=docsdotrs)](https://docs.rs/spacedb-console)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-MIT)
[![Remade With Rust](https://img.shields.io/badge/Remade%20With-Rust-000?logo=rust&logoColor=fff)](https://github.com/Remade-With-Rust)
[![By Mata Network](https://img.shields.io/badge/by-Mata%20Network-5b2be0)](https://www.mata.network/)

**The operator's read-model.**

The substance of an operations console is not pixels — it is the logic that
turns raw fleet / access / audit / settlement observations into the four boards
a SpaceDB business is actually run from. That logic lives here, native and
fully testable; a Dioxus/WASM shell only binds it to a UI.

Part of [SpaceDB](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/README.md). Dual-licensed **MIT OR Apache-2.0**.

```toml
[dependencies]
spacedb-console = "0.5"
```

## The four boards

| Board | Type | Answers |
|---|---|---|
| **Fleet Health** | `FleetHealth` | under-replicated / at-risk / lost shards, quorum loss, replica lag → one `Green` / `Amber` / `Red` rollup |
| **Access & Audit** | `AccessOverview` | human vs AI-agent capabilities, upcoming expirations, revocations, per-agent activity, denials |
| **Economics** | `Economics` | revenue, spend per customer, per-rail split, agent budget burn-down, unsettled claims |
| **Alerts** | `Vec<Alert>` | the page-me surface, severity-sorted |

## Usage

```rust
use spacedb_console::{Config, Dashboard, Observations};
let observations = Observations::default();   // fed from your live adapters
let now = 1_700_000_000;

let dash = Dashboard::assemble(&observations, &Config::at(now));
println!("{}", dash.render_text());

if dash.critical_count() > 0 { /* page someone */ }
```

Each board also has a standalone entry point — `assess_fleet`, `rollup_access`,
`rollup_economics`, `derive_alerts` — so you can compute just the one you need.

`AlertKind` names exactly what went wrong: `ShardLost`, `QuorumLost`,
`ShardAtRisk`, `ShardUnderReplicated`, `ShardOverReplicated`, `HomeOffline`,
`ReplicaLagHigh`, `AgentBudgetExhausted`, `AgentBudgetLow`. `AlertThresholds`
and `Config::expiry_window` are operator-tunable.

## Binding it to a UI

```rust,ignore
// in the Dioxus shell (compiles to wasm32; not part of the tested core):
let dash = Dashboard::assemble(&observations, &Config::at(now));
rsx! {
    HealthBadge { status: dash.health.status }
    for alert in dash.alerts { AlertRow { alert } }
    EconomicsPanel { economics: dash.economics }
}
```

## Why observation DTOs

The console is fed `Observations` — plain `HomeObs` / `ShardObs` / `StrongObs` /
`LagObs` / `CapabilityObs` / `AuditObs` / `SettledObs` / `AgentBudgetObs`
structs — rather than reaching into live subsystems. That keeps the read-model
decoupled from every layer it reports on, and makes every board assertable from
a fixture. [`spacedb-sim`](https://github.com/Remade-With-Rust/spacedb/tree/HEAD/spacedb-sim) emits the same DTOs, so you can
drive the dashboard from a simulated fleet.

## Open-core boundary

Depends on [`spacedb-meter`](https://github.com/Remade-With-Rust/spacedb/tree/HEAD/spacedb-meter) (for `Resource`) and `serde`.
No MATA crate.

## Testing

The workspace defaults to `wasm32`; this crate is native. Test on your host
triple:

```bash
cargo test -p spacedb-console --target aarch64-apple-darwin   # or your host triple
cargo run  -p spacedb-console --example snapshot --target aarch64-apple-darwin
```

Suite: `console.rs`. The `snapshot` example prints a full assembled dashboard.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-MIT) and
[LICENSE-APACHE](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-APACHE).
