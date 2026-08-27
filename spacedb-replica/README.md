# spacedb-replica

[![crates.io](https://img.shields.io/crates/v/spacedb-replica?logo=rust)](https://crates.io/crates/spacedb-replica)
[![docs.rs](https://img.shields.io/docsrs/spacedb-replica?logo=docsdotrs)](https://docs.rs/spacedb-replica)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-MIT)
[![Remade With Rust](https://img.shields.io/badge/Remade%20With-Rust-000?logo=rust&logoColor=fff)](https://github.com/Remade-With-Rust)
[![By Mata Network](https://img.shields.io/badge/by-Mata%20Network-5b2be0)](https://www.mata.network/)

**SpaceDB Layer 2 (hot path) — live convergence between replicas.**

The anti-entropy sync protocol: replicas exchange state vectors and the deltas
those imply, so a write on one replica reaches another live, and a partitioned
link recovers with **zero lost writes** simply by announcing again after it
heals.

Part of [SpaceDB](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/README.md). Dual-licensed **MIT OR Apache-2.0**.

```toml
[dependencies]
spacedb-replica = "0.5"
```

## The model

```rust
use spacedb_crdt::CrdtDoc;
use spacedb_replica::{connected_pair, SyncSession};

let (ta, tb, link) = connected_pair();
let a = SyncSession::new(CrdtDoc::new(10), ta);
let b = SyncSession::new(CrdtDoc::new(20), tb);

a.doc().set_register("title", &"hello".to_string()).unwrap();

// Announce your frontier, then pump until nothing more moves.
a.announce().unwrap();
b.announce().unwrap();
while a.pump().unwrap() + b.pump().unwrap() > 0 {}

assert_eq!(b.doc().get_register::<String>("title").unwrap(), Some("hello".into()));

// Partition and heal — announce again, no writes are lost.
link.partition();
a.doc().increment("views", 5);
link.heal();
```

- `announce()` publishes this replica's state vector.
- `pump()` drains inbound messages, answers with deltas, merges what arrives,
  and returns how much progress it made — `0` means quiescent.

## Honest freshness

`SyncSession::freshness()` never guesses:

| Verdict | Meaning |
|---|---|
| `Live` | connected and caught up to the peer's last-announced frontier |
| `Stale { lag_ops }` | connected, but behind by `lag_ops` operations |
| `Unsynced` | connected, never reconciled — no peer frontier observed yet |
| `Partitioned` | the transport reports the link down |

That verdict is what lets the layers above refuse to call a local-only write
durable.

## Replica roles

`ReplicaRole` distinguishes what a node actually holds: `Full` (a Home Computer
— whole dataset, serves reads and on-node compute), `Partial` (a phone's working
subset, offline-first for what it has, described by a `SubsetSpec`), and a
buyer-only client that holds nothing and queries the nearest full replica.

## The transport seam

`Transport` is where the network lives. This crate ships `InProcessTransport`
plus a `Link` with a partition switch, so the whole protocol — including
partition recovery — is provable inside one process with no sockets.

- **MATA** implements `Transport` over its iroh + relay + roster-auth stack
  (`mata-sync`).
- **A self-hoster** can implement it over plain `iroh`, or anything that moves
  bytes between two peers.

`SyncMessage::encode` / `decode` give you the framed wire form to put on it.

## Open-core boundary

Depends only on [`spacedb-crdt`](https://github.com/Remade-With-Rust/spacedb/tree/HEAD/spacedb-crdt). No MATA crate is referenced.

## Testing

The workspace defaults to `wasm32`; this crate is native. Test on your host
triple:

```bash
cargo test -p spacedb-replica --target aarch64-apple-darwin   # or your host triple
```

Suites: `sync.rs` (propagation, bidirectional convergence, partition heal),
`state.rs` (freshness), `roles.rs`, `reactive_sync.rs`.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-MIT) and
[LICENSE-APACHE](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-APACHE).
