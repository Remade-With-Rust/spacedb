# spacedb-store

[![crates.io](https://img.shields.io/crates/v/spacedb-store?logo=rust)](https://crates.io/crates/spacedb-store)
[![docs.rs](https://img.shields.io/docsrs/spacedb-store?logo=docsdotrs)](https://docs.rs/spacedb-store)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-MIT)
[![Remade With Rust](https://img.shields.io/badge/Remade%20With-Rust-000?logo=rust&logoColor=fff)](https://github.com/Remade-With-Rust)
[![By Mata Network](https://img.shields.io/badge/by-Mata%20Network-5b2be0)](https://www.mata.network/)

**SpaceDB Layer 0 — the per-node storage primitive.**

A typed, transactional, order-preserving key/value store that everything else in
SpaceDB rests on. Getting this small and correct is the whole game: every layer
above inherits its guarantees.

Part of [SpaceDB](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/README.md). Dual-licensed **MIT OR Apache-2.0**.

```toml
[dependencies]
spacedb-store = "0.5"
```

## The model

```rust
use spacedb_store::{Durability, KvEngine, MemEngine, Table, WriteTx};

let engine = MemEngine::new();
let users: Table<u64, String> = Table::new("users");

// One write transaction spans many tables and commits all-or-nothing.
let mut w = engine.begin_write(Durability::Immediate).unwrap();
users.put(&mut w, &42, &"ada".to_string()).unwrap();
w.commit().unwrap();

let r = engine.begin_read().unwrap();
assert_eq!(users.get(&r, &42).unwrap(), Some("ada".to_string()));
```

`Table<K, V>` applies both codecs exactly once, so no layer above ever touches
raw bytes.

## What's here

| Module | What it gives you |
|---|---|
| [`engine`](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/spacedb-store/src/engine.rs) | The `KvEngine` seam — `ReadTx` / `WriteTx` / `Readable`, `Durability` |
| [`redb_engine`](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/spacedb-store/src/redb_engine.rs) | `RedbEngine` — the durable engine (native targets; `redb` 2.x) |
| [`mem_engine`](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/spacedb-store/src/mem_engine.rs) | `MemEngine` — in-memory, identical transaction semantics, for tests |
| [`codec`](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/spacedb-store/src/codec.rs) | Deterministic `postcard` value codec + an order-preserving key codec |
| [`table`](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/spacedb-store/src/table.rs) | `Table<K, V>` — the typed primitive |
| [`collection`](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/spacedb-store/src/collection.rs) | `Collection` — a schema-versioned namespace over tables |
| [`crypto`](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/spacedb-store/src/crypto.rs) | The AEAD row boundary: `seal_row` / `open_row`, DEK wrap / unwrap / rewrap |
| [`meta`](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/spacedb-store/src/meta.rs) | `_meta` store-version gate — refuses to open a store from a newer schema |
| [`extern_value`](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/spacedb-store/src/extern_value.rs) | Large-value externalization: `classify`, `content_hash`, `ExternRef` |

## Guarantees

- **Atomic multi-table writes.** One `WriteTx` spans many tables and commits
  all-or-nothing; dropping it rolls back.
- **Order-preserving keys.** `a < b ⟺ encode(a) < encode(b)`, so range scans
  return logical order. The key codec is a verified bijection (proptest).
- **Single-writer / snapshot reads**, identical across both engines — the
  `tests/engines.rs` suite runs the same assertions against each.
- **Encrypted at the value boundary.** Rows are sealed with a per-collection DEK
  (AES-GCM); the DEK itself is wrapped by a key from the `KeyProvider` seam, so
  the storage engine never sees plaintext or the master key.
- **Crash-safe.** `tests/crash.rs` kills the `spacedb_crash_helper` binary
  mid-commit and asserts the store reopens with no torn write.

## Seams an operator fills

- **`KvEngine`** — swap the storage engine. Two implementations ship.
- **`KeyProvider`** — where the wrapping key comes from. MATA binds it to the
  device vault; a self-hoster can supply a passphrase-derived key.

## Open-core boundary

Depends on **no** MATA crate. MATA-specific capability (vault key, identity,
mesh replication, settlement) enters only through seams this crate defines. The
dependency arrow is MATA → SpaceDB, never the reverse.

## Testing

The workspace defaults to `wasm32`; this crate is native. Test on your host
triple:

```bash
cargo test -p spacedb-store --target aarch64-apple-darwin   # or your host triple
```

Suites: `engines.rs` (shared semantics), `encrypted.rs` (AEAD boundary),
`meta.rs` (version gate), `crash.rs` (kill-mid-commit durability), plus
proptest round-trips for both codecs.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-MIT) and
[LICENSE-APACHE](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-APACHE).
