# spacedb-meter

[![crates.io](https://img.shields.io/crates/v/spacedb-meter?logo=rust)](https://crates.io/crates/spacedb-meter)
[![docs.rs](https://img.shields.io/docsrs/spacedb-meter?logo=docsdotrs)](https://docs.rs/spacedb-meter)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-MIT)
[![Remade With Rust](https://img.shields.io/badge/Remade%20With-Rust-000?logo=rust&logoColor=fff)](https://github.com/Remade-With-Rust)
[![By Mata Network](https://img.shields.io/badge/by-Mata%20Network-5b2be0)](https://www.mata.network/)

**SpaceDB Layer 6 — metering & settlement.**

SpaceDB **measures**; it does not price or pay. This crate computes the three
resource amounts deterministically, accumulates them, and drains them into
amounts-only claims. What a claim is *worth*, and who actually gets paid, sits
behind a seam.

Part of [SpaceDB](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/README.md). Dual-licensed **MIT OR Apache-2.0**.

```toml
[dependencies]
spacedb-meter = "0.5"
```

## What gets measured

```rust
use spacedb_meter::Usage;
let (bytes, seconds, replica_count) = (1u64 << 30, 86_400u64, 3u32);
let (fuel, invocations) = (2_500_000u64, 40u64);
let (server_claimed, consumer_acked) = (1_000_000u64, 999_000u64);

Usage::storage(bytes, seconds, replica_count);  // byte-seconds × replicas
Usage::compute(fuel, invocations);              // deterministic fuel + per-call
Usage::transit(server_claimed, consumer_acked); // the MINIMUM of the two
```

Transit takes the **minimum** of what the server claims it served and what the
consumer acknowledges receiving. Bilateral corroboration means neither side can
inflate a bill unilaterally — over-claiming simply doesn't pay.

## Ledger → claims

```rust
use spacedb_meter::{MeterLedger, Usage};
let (period_start, period_end) = (1_700_000_000u64, 1_700_086_400u64);

let mut ledger = MeterLedger::new();
ledger.record("did:mata:customer", Usage::storage(1 << 30, 86_400, 3));
ledger.record("did:mata:customer", Usage::compute(2_500_000, 40));

// Drain a billing period into per-customer, per-class claims.
let claims = ledger.drain_claims("did:mata:node", period_start, period_end);
```

A `UsageClaim` carries a deterministic `claim_id` (`node:settles_to:class:
period_end`) — so a retried or duplicated claim dedups instead of double-billing
— the amounts, the period bounds, and an optional `ProofRef`
(`StorageProbe` / `ComputeAttestation` / `TransitReceipt`) content-addressing
the artifact that backs it, e.g. a `FunctionRun` digest from
[`spacedb-query`](https://github.com/Remade-With-Rust/spacedb/tree/HEAD/spacedb-query).

## Pricing and budgets

`RateCard` prices storage per GiB-month, compute per megafuel plus per
invocation, and transit per GiB — multiplying before dividing so sub-unit usage
is never silently rounded to zero. `estimate(&usages)` gives a pre-deploy cost
before anything runs.

`Budget` is the agent spend cap: `can_afford`, `charge` (fails rather than
overdrawing), `credit`, `remaining`. An AI agent with a capability gets a
hard ceiling, not a monthly surprise.

## The settlement seam

```rust
use spacedb_meter::{MeterError, Settled, UsageClaim};
pub trait Settlement {
    fn settle(&mut self, claim: &UsageClaim) -> Result<Settled, MeterError>;
}
```

- **`LocalSettlement`** ships here: price against a rate card, tally per
  customer. Enough for a self-hoster; no tokens minted.
- **A host** implements `Settlement` over its own money plane. For MATA that is
  the existing `UsageClaim → Maestro counter-sign → EarningRecord → Iron Bank`
  loop — so SpaceDB usage settles through it without ever depending on it.

## Open-core boundary

Depends on `serde` / `postcard` / `thiserror` only — no `spacedb-*` and no
`mata-*` crates.

## Testing

The workspace defaults to `wasm32`; this crate is native. Test on your host
triple:

```bash
cargo test -p spacedb-meter --target aarch64-apple-darwin   # or your host triple
```

Suite: `meter.rs` — measurement determinism, transit corroboration, claim
dedup keys, rate-card rounding, budget exhaustion.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-MIT) and
[LICENSE-APACHE](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-APACHE).
