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
spacedb-store = "0.7"
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

## Performance (0.7.3)

`ir_redb` 330.43M -> 329.51M (-0.28%), `ir_store` 191.17M -> 190.99M (-0.09%),
outputs unchanged:

- rusty_zstd 0.4.2 (from 0.4.1). Rows compress through its new
  `compress_into_with` into one buffer per thread, copied into the row only
  when shorter; no allocation per compressed row. Stored bytes are identical
  to 0.7.2's.

## Performance (0.7.2)

Measured on exact instruction counts (callgrind) with deterministic drivers
whose outputs did not change; release build, 0.7.1 -> 0.7.2:
`ir_store` 194.1M -> 191.2M (-1.5%), `ir_redb` 335.0M -> 330.4M (-1.4%), `ir_txrange` 11.6M -> 7.4M (**-36.1%**).

- `Readable::range_with` lends each row to a visitor instead of collecting copies; `Table::range` and `Collection::range` use it, and a collection scan reuses one row buffer and one decompression buffer.
- `Readable::contains_raw`: presence checks (`contains_key`, `delete_raw`) no longer copy the row.
- `Collection::put_by` / `contains_key_by` / `get_with_by` take a borrowed key (`&str` for a `String` key).
- redb read and write transactions open each table once, on first use, instead of once per row; the write side holds its tables with `self_cell`, no unsafe code of ours. The transaction types stay `Send` and `Sync`.
- The in-memory engine's overlay borrows the table name on put and delete.
- Rows are compressed without zstd's content checksum (the AEAD tag already authenticates them); checksummed rows from earlier versions still read.

## Performance (0.7.0)

Measured on exact instruction counts (callgrind) with this crate's
deterministic driver, `examples/ir_store.rs`; outputs unchanged by every change.
Release build, 0.6.0 -> 0.7.0: **416.9M -> 356.1M instructions (-14.6%)**.

- A put seals its row in one buffer: the value is encoded straight after the
  nonce slot, packed and encrypted in place, and moved into the engine
  (`WriteTx::put_raw_owned`) rather than copied.
- A get decrypts in place and decodes straight out of the engine's buffer;
  uncompressed rows are borrowed, not copied.
- The per-row AAD is built on the stack; a range scan builds one cipher; the
  DEK is unwrapped into its zeroizing array without a heap plaintext.
- Keys are escaped and unescaped a run at a time, not a byte at a time.
- `Bytes`: a byte-string value type that stores exactly the same rows as
  `Vec<u8>` but serializes as one copy instead of one call per byte. A
  `Collection<K, Bytes>` can lend a value without copying it (`get_with`).
- New: `Collection::contains_key` checks presence from the engine alone.
- rusty_zstd 0.3.0 (byte-identical output at the default level 3).

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
