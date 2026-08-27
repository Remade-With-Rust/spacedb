# spacedb-durability

[![crates.io](https://img.shields.io/crates/v/spacedb-durability?logo=rust)](https://crates.io/crates/spacedb-durability)
[![docs.rs](https://img.shields.io/docsrs/spacedb-durability?logo=docsdotrs)](https://docs.rs/spacedb-durability)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-MIT)
[![Remade With Rust](https://img.shields.io/badge/Remade%20With-Rust-000?logo=rust&logoColor=fff)](https://github.com/Remade-With-Rust)
[![By Mata Network](https://img.shields.io/badge/by-Mata%20Network-5b2be0)](https://www.mata.network/)

**SpaceDB Layer 2 (cold path) — mesh durability.**

Durability that survives any single home dying: a dataset is sealed into a
content-addressed, Reed–Solomon erasure-coded snapshot whose shards spread
across diverse homes, so losing homes is recoverable rather than fatal.

Part of [SpaceDB](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/README.md). Dual-licensed **MIT OR Apache-2.0**.

```toml
[dependencies]
spacedb-durability = "0.5"
```

## The model

```rust
use spacedb_durability::{encode_snapshot, reconstruct_snapshot};
let snapshot_bytes: Vec<u8> = (0..4096u32).map(|i| i as u8).collect();

// 6 data + 3 parity: any 6 of the 9 shards reconstruct the snapshot.
let (manifest, shards) = encode_snapshot(&snapshot_bytes, 6, 3).unwrap();

// Three homes die. The remaining six still rebuild it, byte-identical.
let survivors: Vec<_> = shards.into_iter().skip(3).collect();
let restored = reconstruct_snapshot(&manifest, &survivors).unwrap();
assert_eq!(restored, snapshot_bytes);
```

The `Manifest` is the verifiable part: it carries every `ShardRef` with its
content hash, so a corrupted or substituted shard is detected during
reconstruction rather than silently folded into the output.

`manifest.shards_needed()` / `total_shards()` / `fault_tolerance()` state the
k-of-n arithmetic; `encode` / `decode` give it a wire form.

## Placement, distribution, health, repair

The full cold-path loop, all deterministic and testable against an in-memory
fleet:

| Function | What it does |
|---|---|
| `allocate(num_shards, &targets)` | anti-affinity placement — spread shards across failure domains |
| `distribute(&manifest, &shards, &placement, &fleet)` | push each shard to its placed node's `ShardStore` |
| `recover(..)` | pull back enough shards from online nodes and reconstruct |
| `health(..)` | `ReplicaHealth` — reachable vs needed, `slack()`, `is_repairable()`, lost / at-risk / under-replicated |
| `repair(..)` | reconstruct → re-encode → re-place the missing shards, honoring anti-affinity |
| `reclaim(..)` | drop surplus copies once a replica is over-replicated |

`Fleet` / `Node` model the homes, with `kill` / `revive` so a test can take
machines down and assert the loop converges back to healthy.

## Seams an operator fills

- **`ShardStore`** — content-addressed `put` / `get` / `has` / `delete`.
  `MemShardStore` ships for tests; MATA implements it over `maestro-disco`.
- **Placement targets** — `TargetInfo` carries the failure-domain label the
  anti-affinity rule spreads across.

## Why this is safe to host on someone else's machine

Erasure coding operates on **opaque, already-encrypted** snapshot bytes. A
hosting home stores shards it cannot read, and holding fewer than `k` of them
reveals nothing at all.

## Open-core boundary

Depends on `reed-solomon-erasure`, `blake3`, `serde`/`postcard` — no MATA crate.

## Testing

The workspace defaults to `wasm32`; this crate is native. Test on your host
triple:

```bash
cargo test -p spacedb-durability --target aarch64-apple-darwin   # or your host triple
```

Suites: `erasure.rs` (incl. proptest round-trips over random k/n and random
survivor sets), `placement.rs`, `repair.rs`, `reclaim.rs`.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-MIT) and
[LICENSE-APACHE](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-APACHE).
