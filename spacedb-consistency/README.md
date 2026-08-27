# spacedb-consistency

[![crates.io](https://img.shields.io/crates/v/spacedb-consistency?logo=rust)](https://crates.io/crates/spacedb-consistency)
[![docs.rs](https://img.shields.io/docsrs/spacedb-consistency?logo=docsdotrs)](https://docs.rs/spacedb-consistency)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-MIT)
[![Remade With Rust](https://img.shields.io/badge/Remade%20With-Rust-000?logo=rust&logoColor=fff)](https://github.com/Remade-With-Rust)
[![By Mata Network](https://img.shields.io/badge/by-Mata%20Network-5b2be0)](https://www.mata.network/)

**SpaceDB Layer 3 — consistency tiers, and the honesty contract.**

Consistency is a **per-field choice**, declared in the schema, because in a
partition-prone world one global setting is always wrong. And every operation
reports the level it **actually** achieved — so an app can never mistake a
local-only write for a durable one, or a lagging read for a current one.

Part of [SpaceDB](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/README.md). Dual-licensed **MIT OR Apache-2.0**.

```toml
[dependencies]
spacedb-consistency = "0.5"
```

## The three tiers

| Tier | Cost | Behavior under partition |
|---|---|---|
| `Tier::Convergent` (default) | free | always available; CRDT auto-merge |
| `Tier::Causal` | cheap, no consensus | available; read-your-writes + monotonic reads within a session |
| `Tier::Strong` | quorum round | **fails safe** — `Unavailable`, never a divergent commit |

```rust
use spacedb_consistency::{ConsistencySchema, Tier};

let schema = ConsistencySchema::new()
    .with_field("bio",      Tier::Convergent)
    .with_field("cursor",   Tier::Causal)
    .with_field("username", Tier::Strong);
assert_eq!(schema.tier_of("username"), Tier::Strong);
assert_eq!(schema.tier_of("unlisted"), schema.default_tier());
```

## The honesty contract

Every op returns an `Outcome`:

| Outcome | Means |
|---|---|
| `Committed { tier }` | durable at the tier you asked for |
| `Local` | written here, converging outward — **not yet durable elsewhere** |
| `Stale { lag }` | a read served from a replica known to be `lag` behind |
| `Unavailable { reason }` | refused rather than guessed (`UnavailableReason`) |

`is_committed()`, `is_available()`, and `tier()` let a caller branch on it
without matching every variant.

## Causal+ sessions

```rust
use spacedb_consistency::CausalSession;
use spacedb_crdt::CrdtDoc;
let doc = CrdtDoc::new(1);

let mut session = CausalSession::new();
session.record_write(&doc);   // pins the frontier this session has seen
let outcome = session.read(&doc);  // Committed if caught up, Stale{lag} if not
```

A session token tracks the state vector this client has observed, giving
read-your-writes and monotonic reads across replicas with **no consensus and no
coordination** — it degrades to an honest `Stale` rather than blocking.

## Strong tier

`QuorumGroup` is a majority-quorum register with the operations that actually
need linearizability:

- `cas(key, expected_version, new_value)` — compare-and-set
- `claim_unique(key, owner)` — globally unique claim (usernames, handles)
- `init_seats(key, count)` / `acquire_seat(key)` / `seats_remaining(key)` —
  bounded resources (license seats, inventory)

Each returns a `StrongResult`: `Committed`, `Rejected(RejectReason)`, or
`Unavailable(..)`. `partition(member)` / `heal(member)` take members offline so
tests can prove the group refuses to commit without a majority rather than
splitting.

## Open-core boundary

Depends only on [`spacedb-crdt`](https://github.com/Remade-With-Rust/spacedb/tree/HEAD/spacedb-crdt). No MATA crate.

## Testing

The workspace defaults to `wasm32`; this crate is native. Test on your host
triple:

```bash
cargo test -p spacedb-consistency --target aarch64-apple-darwin   # or your host triple
```

Suites: `causal.rs` (read-your-writes, monotonic reads), `strong.rs` (quorum,
CAS, unique claims, seats, partition fail-safe).

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-MIT) and
[LICENSE-APACHE](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-APACHE).
