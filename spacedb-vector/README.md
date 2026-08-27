# spacedb-vector

[![crates.io](https://img.shields.io/crates/v/spacedb-vector?logo=rust)](https://crates.io/crates/spacedb-vector)
[![docs.rs](https://img.shields.io/docsrs/spacedb-vector?logo=docsdotrs)](https://docs.rs/spacedb-vector)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-MIT)
[![Remade With Rust](https://img.shields.io/badge/Remade%20With-Rust-000?logo=rust&logoColor=fff)](https://github.com/Remade-With-Rust)
[![By Mata Network](https://img.shields.io/badge/by-Mata%20Network-5b2be0)](https://www.mata.network/)

**SpaceDB Layer 4 — the private-RAG substrate.**

A native, on-node vector index co-located with the data: a query embedding goes
in, and **only the top-k results come out**. The corpus never leaves the home.

Part of [SpaceDB](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/README.md). Dual-licensed **MIT OR Apache-2.0**.

```toml
[dependencies]
spacedb-vector = "0.5"
```

## The model

```rust
use spacedb_vector::{retrieve, Metric, VectorIndex};
use spacedb_access::Decision;
let embedding_a: Vec<f32> = (0..384).map(|i| i as f32).collect();
let embedding_b: Vec<f32> = (0..384).map(|i| (384 - i) as f32).collect();
let query_embedding = embedding_a.clone();
let decision = Decision::Allow;   // from spacedb-access `authorize`

let mut index = VectorIndex::new(384, Metric::Cosine);
index.insert("note-1", embedding_a).unwrap();
index.insert("note-2", embedding_b).unwrap();

// An AI agent's retrieval is gated by an mID capability decision.
let hits = retrieve(&index, &query_embedding, 5, &decision).unwrap();
for m in hits {
    println!("{} {:.3}", m.id, m.score);   // ids + scores — nothing else
}
```

`retrieve` refuses with `VectorError::Denied` unless the
[`spacedb-access`](https://github.com/Remade-With-Rust/spacedb/tree/HEAD/spacedb-access) `Decision` allows it. That is the concrete
mechanism behind *"inaccessible by default, accessible by mID-gated consent"*:
an authorized agent gets semantic access to **results**, not bulk data.

Metrics: `Cosine` (magnitude-invariant, `[-1, 1]`), `Dot`, and `Euclidean`
(negated, so higher is always better and `k`-selection is uniform).

## What ships today

An exact flat k-NN index — `insert` / `remove` / `search`, dimension-checked on
every operation. Sub-linear ANN (HNSW / IVF) is a future optimization **behind
the same surface**: exactness first, so correctness is never traded for a
recall knob nobody measured.

## Open-core boundary

Depends only on [`spacedb-access`](https://github.com/Remade-With-Rust/spacedb/tree/HEAD/spacedb-access), for the capability gate.
No MATA crate.

## Testing

The workspace defaults to `wasm32`; this crate is native. Test on your host
triple:

```bash
cargo test -p spacedb-vector --target aarch64-apple-darwin   # or your host triple
```

Suite: `vector.rs` — metric correctness, top-k ordering, dimension mismatch, and
the denied-without-capability path.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-MIT) and
[LICENSE-APACHE](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-APACHE).
