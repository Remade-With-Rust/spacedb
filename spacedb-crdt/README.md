# spacedb-crdt

[![crates.io](https://img.shields.io/crates/v/spacedb-crdt?logo=rust)](https://crates.io/crates/spacedb-crdt)
[![docs.rs](https://img.shields.io/docsrs/spacedb-crdt?logo=docsdotrs)](https://docs.rs/spacedb-crdt)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-MIT)
[![Remade With Rust](https://img.shields.io/badge/Remade%20With-Rust-000?logo=rust&logoColor=fff)](https://github.com/Remade-With-Rust)
[![By Mata Network](https://img.shields.io/badge/by-Mata%20Network-5b2be0)](https://www.mata.network/)

**SpaceDB Layer 1 — convergent collections.**

The default data tier, and the reason SpaceDB survives a partition-prone mesh:
data is modeled as **Y-CRDT** (via [`yrs`](https://crates.io/crates/yrs)), so
every write is locally available and merges conflict-free with no coordination.
Writing offline on a Starlink-partitioned home is a non-event, not an error.

Part of [SpaceDB](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/README.md). Dual-licensed **MIT OR Apache-2.0**.

```toml
[dependencies]
spacedb-crdt = "0.5"
```

## The model

A `CrdtDoc` is a document with a typed field → CRDT-type mapping. Pick the CRDT
per field; the merge rule follows from the type.

```rust
use spacedb_crdt::CrdtDoc;

let laptop = CrdtDoc::new(1);   // actor id
let phone  = CrdtDoc::new(2);

laptop.set_register("display_name", &"Ada").unwrap();  // LWW-Register
laptop.increment("visits", 1);                          // PN-Counter
laptop.text_push("bio", "building on SpaceDB");         // Y.Text
laptop.set_add("tags", "rust");                         // OR-Set

// Sync: exchange state vectors, ship only the delta they imply.
let delta = laptop.encode_update_since(&phone.state_vector()).unwrap();
phone.apply_update(&delta).unwrap();

assert_eq!(phone.get_register::<String>("display_name").unwrap().as_deref(), Some("Ada"));
assert_eq!(phone.counter("visits"), 1);
```

> **Direct peer-to-peer only.** `encode_update_since` is safe between the two
> replicas doing the merge. Do **not** re-encode deltas through a third "relay"
> doc (A → relay → B): for some actor-id orderings — exactly the orderings
> hash-derived device ids produce — the relay recomputes the delta from its own
> re-ordered state and can silently **drop a record**. For relayed topologies,
> ship each replica's raw local update bytes (`take_local_updates`) through an
> append-only log and apply them verbatim and idempotently; a relay keeps a
> queryable copy by replaying the same log. The rustdoc on `encode_update_since`
> and `tests/relay.rs` carry the full story.

| CRDT type | Field API | Merge rule |
|---|---|---|
| LWW-Register | `set_register` / `get_register` / `remove_register` | last writer wins |
| PN-Counter | `increment` / `counter` | sum of per-actor increments |
| Y.Text | `text_push` / `text_insert` / `text_remove` / `text` | character-level intent preservation |
| OR-Set | `set_add` / `set_remove` / `set_contains` / `set_members` | add wins over concurrent remove |

## Reactive queries

```rust
use spacedb_crdt::CrdtDoc;
let doc = CrdtDoc::new(1);
let watcher = doc.watch();
// ... after any local or merged write:
if watcher.drain_changed() { /* re-render */ }
```

`ReactiveQuery::poll` gives the same thing with a derived value: it recomputes
only when the document revision moves.

## Encrypted persistence

`CrdtStore` writes documents into a [`spacedb-store`](https://github.com/Remade-With-Rust/spacedb/tree/HEAD/spacedb-store) engine
behind the `KeyProvider` AEAD boundary — `save`, `load`, `apply_remote`,
`contains`. `compact_updates` folds an update log into one minimal update so a
long-lived document doesn't grow without bound.

## The convergence property

*The same updates, applied in any order, produce the same state.* That is the
guarantee everything above this layer depends on, and it is proven — not
asserted — by the fuzzed suite in [`tests/convergence.rs`](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/spacedb-crdt/tests/convergence.rs),
which shuffles update orderings across replicas and requires byte-identical
final state.

`ops_behind` and `estimated_state_size` expose honest lag / size, which
[`spacedb-replica`](https://github.com/Remade-With-Rust/spacedb/tree/HEAD/spacedb-replica) turns into a `Freshness` verdict.

## Open-core boundary

Depends on `yrs` and `spacedb-store` — never on a MATA crate.

## Testing

The workspace defaults to `wasm32`; this crate is native. Test on your host
triple:

```bash
cargo test -p spacedb-crdt --target aarch64-apple-darwin   # or your host triple
```

Suites: `convergence.rs` (fuzzed), `fields.rs`, `reactive.rs`, `persistence.rs`,
`compact.rs`, `lag.rs`.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-MIT) and
[LICENSE-APACHE](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-APACHE).
