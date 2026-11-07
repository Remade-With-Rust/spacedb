## In The Wild with 14,019 Active Installs

> [MATA Network](https://mata.network) is hosting the greatest Disco Party on Earth. A Disco Party is a
> sovereign distributed cloud and digital freedom toolkit by MATA enabling ownership and accessibility of data.

> [DLDeploy.com](https://DLDEPLOY.COM) uses over a dozen Remade with Rust portfolio tools to provide browser based
> AI, and deploys the entire rusty_ESP portfolio for zero-C Espressif technology on ESP-32.

> [RAG Converter](https://ragconverter.com) is a browser based, completely local AI tool deploying Mercury and Carmenta to do local,
> realtime conversion of business documents to RAG data. API built for businesses to make emails, pdf, images, and videos available to AI.

> Build the new internet with our <a href="https://drive.google.com/drive/folders/1VjvP1zSJWH1DmkTqL1qEPt3OM8Ub8jPX?usp=drive_link">AI skills for Rust</a>. Create on any platform with security by design.

---

# SpaceDB

[![crates.io](https://img.shields.io/crates/v/spacedb-sdk?logo=rust&label=spacedb-sdk)](https://crates.io/crates/spacedb-sdk)
[![docs.rs](https://img.shields.io/docsrs/spacedb-sdk?logo=docsdotrs)](https://docs.rs/spacedb-sdk)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-MIT)
[![Remade With Rust](https://img.shields.io/badge/Remade%20With-Rust-000?logo=rust&logoColor=fff)](https://github.com/Remade-With-Rust)
[![By Mata Network](https://img.shields.io/badge/by-Mata%20Network-5b2be0)](https://www.mata.network/)

**A local-first, CRDT-native, mesh-replicated database with no data center.**

Your app's data lives encrypted across machines near your users — available
offline, converging automatically, with compute running next to the data and
every access gated by a signed, scoped, revocable capability (for humans **and**
AI agents).

SpaceDB is **open source** — dual-licensed **MIT OR Apache-2.0**. Embed it in your app,
run it on your own devices, sync your own replicas: free, forever, no strings. The crates
depend on nothing proprietary.

**When you want to _distribute_ — reach users you don't own, durably, and get paid for it —
that's [MATA disco](https://disco.mata.network).** SpaceDB defines the seams (transport,
shard store, key directory, settlement); MATA runs the mesh of homes, the placement + repair
network, mID access, and `$MATA` settlement behind them. The library is yours; the _network_
is the product. So: **integrate locally with SpaceDB, distribute with disco.**

## Quick start

Add the SDK — it's the one crate you need; it composes the whole stack **and**
installs a hardened, pure-Rust allocator by default:

```toml
[dependencies]
spacedb-sdk = "0.7"
```

> **Writing a library rather than an application?** Add
> `default-features = false` — see [Memory allocation](#memory-allocation-rusty_alloc).

```rust
use spacedb_sdk::{
    Database, Schema, CrdtType, Tier, Identity, Capability, SignedCapability,
    Scope, Ops, Outcome, StrongResult,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Open an offline-first local replica for this device.
    let mut db = Database::open(Identity::generate("did:mata:home-1")?);

    // 2. Declare a schema: each field picks its CRDT type AND its consistency tier.
    db.define(
        Schema::new("profile")
            .field("bio",          CrdtType::Text,     Tier::Convergent) // auto-merges
            .field("display_name", CrdtType::Register, Tier::Convergent)
            .field("cursor",       CrdtType::Register, Tier::Causal)     // read-your-writes
            .field("visits",       CrdtType::Counter,  Tier::Convergent)
            .field("username",     CrdtType::Register, Tier::Strong),    // globally unique
    );

    // 3. The owner grants a capability — to a person or an AI agent — scoped,
    //    expiring, revocable, with its own spend budget.
    let owner = Identity::generate("did:mata:owner")?;
    db.register_identity(&owner)?;
    db.set_clock(1_700_000_000);

    let cap = Capability::grant(
            owner.did().clone(),
            "did:agent:assistant",                 // an AI agent mID
            Scope::Collection("profile".into()),
            Ops::READ | Ops::WRITE,
        )?
        .with_expiry(1_702_592_000)
        .with_budget(1_000_000);                   // micro-$MATA it may spend
    let mut session = db.session(SignedCapability::sign(cap, &owner)?);

    // 4. Write offline. Every op returns the consistency it ACTUALLY achieved.
    let outcome = db.put_register(&mut session, "profile", "display_name", "Ada")?;
    assert_eq!(outcome, Outcome::Local);           // durable here, converging outward
    db.increment(&mut session, "profile", "visits", 1)?;
    db.append_text(&mut session, "profile", "bio", "building on SpaceDB")?;

    // 5. Read it back — honest about freshness.
    let (name, read) = db.read_register(&mut session, "profile", "display_name")?;
    println!("{:?} ({:?})", name, read);           // Some("Ada") (Committed(Convergent))

    // 6. Strong tier when you need it: globally unique, or it cleanly refuses.
    match db.claim_unique(&mut session, "profile", "username", "ada")? {
        StrongResult::Committed       => println!("username is yours"),
        StrongResult::Rejected(_)     => println!("already taken"),
        StrongResult::Unavailable(_)  => println!("no quorum right now — try later"),
    }

    Ok(())
}
```

That's the whole model: **open → schema → grant → write/read with honest state →
strong when you mean it.** No connection string, no server, no network required.

### Sync two replicas (still no server)

```rust
let bytes = laptop.export("profile");     // CRDT state, content-addressed
phone.import("profile", &bytes)?;         // merges; conflicts resolve by CRDT rules
```

### React to change

```rust
let watcher = db.watch("profile");
// ... after any local or merged write:
if watcher.drain_changed() { /* re-render */ }
```

---

## Memory allocation (`rusty_alloc`)

**`spacedb-sdk` installs [`rusty_alloc`](https://github.com/remade-with-rust/rusty_alloc)
as the process-wide allocator by default, and we highly recommend leaving it on
for any distributed deployment.**

SpaceDB is a *distributed* database: replicas routinely run on machines their
operator does not control. On that hardware the allocator is part of the failure
surface, not an implementation detail:

| property | why it matters on a node you don't own |
|---|---|
| **Double free aborts** instead of corrupting the heap (~0.4% overhead) | A memory bug becomes a crash you can see, rather than silent divergence you cannot trust. A replica that corrupts its own heap is a replica that lies to the mesh. |
| **Pure Rust — no C allocator in the tree** | The allocator underneath a database holding other people's encrypted data is code we can audit, and it is the last major C dependency `std` drags in. |
| **`secure` feature**: guard pages + encrypted free lists | Adds ~4–7% instructions (measured) and is worth it for nodes taking untrusted input. |
| **One allocator across every target** | It runs on Linux, macOS, Windows and `wasm32` (no emscripten), so a node behaves the same wherever it is placed. |

An unconfigured node should be the hardened one, so this is opt-**out**:

```toml
# Bring your own allocator (jemalloc, mimalloc, the system one, …)
spacedb-sdk = { version = "0.7", default-features = false }
```

That genuinely removes `rusty_alloc` from the dependency graph — not merely from
a `cfg` — leaving you free to declare your own `#[global_allocator]`. The full
API is re-exported either way.

For the hardened node profile:

```toml
spacedb-sdk = { version = "0.7", features = ["secure"] }
```

### Library authors: opt out

```toml
[dependencies]
spacedb-sdk = { version = "0.7", default-features = false }
```

This is a real Cargo behaviour, not a style preference. A program may contain
exactly **one** `#[global_allocator]`, and Cargo features are **additive across
the entire dependency graph**. If a *library* depended on `spacedb-sdk` with
default features, every application using it would inherit this allocator — and
any application that had already chosen its own would fail to build with:

```text
error: the `#[global_allocator]` in this crate conflicts with global allocator in: spacedb_sdk
```

…which it could not fix from its own manifest. **Applications** decide the
allocator; **libraries** should stay out of that decision. MATA's own crates do
exactly this — the workspace pins `spacedb-sdk` with `default-features = false`
and installs the allocator at the binary instead.

## The layers

One crate per layer; `spacedb-sdk` ties them together. Each defines a **seam** an
operator implements for the hosted product — the dependency arrow is
**MATA → SpaceDB**, never the reverse.

| Crate | Layer | What it gives you | Seam an operator fills |
|---|---|---|---|
| [`spacedb-store`](spacedb-store/) | L0 storage | encrypted KV, typed tables | `KvEngine`, `KeyProvider` (vault) |
| [`spacedb-crdt`](spacedb-crdt/) | L1 data | convergent docs, reactive queries | — |
| [`spacedb-replica`](spacedb-replica/) | L2 sync | anti-entropy sync, honest freshness | `Transport` (iroh/relay) |
| [`spacedb-durability`](spacedb-durability/) | L2 mesh | erasure shards, placement, self-repair | `ShardStore`, placement |
| [`spacedb-access`](spacedb-access/) | L5 access | mID capabilities, delegation, audit | `KeyDirectory` (mID/IAMHUMAN) |
| [`spacedb-query`](spacedb-query/) | L4 compute | deterministic compute-to-data + attest | redundant placement |
| [`spacedb-vector`](spacedb-vector/) | L4 RAG | on-node top-k; corpus never leaves | — |
| [`spacedb-consistency`](spacedb-consistency/) | L3 tiers | convergent / causal+ / strong | strong-tier placement |
| [`spacedb-meter`](spacedb-meter/) | L6 economics | metering, pricing, budgets | `Settlement` → Iron Bank |
| [`spacedb-sdk`](spacedb-sdk/) | — | **the developer surface** (above) — hardened allocator by default; `default-features = false` to bring your own | — |
| [`spacedb-console`](spacedb-console/) | — | operator dashboard read-model | live observation adapters |
| [`spacedb-sim`](spacedb-sim/) | — | deterministic digital twin (testing) | — |

Each crate's README covers its own API, guarantees, and seams.

---

## Status — Phase 1 complete (performance passes in 0.7.0, 0.7.2 and 0.7.3)

All eight milestones built, **fully tested on the native target, zero warnings**:

- **M1 `spacedb-store`** — `KvEngine` seam (redb + in-memory), order-preserving
  key codec, typed `Table<K,V>`, AEAD value boundary (per-collection DEK via a
  `KeyProvider` seam), `_meta` migration gate, kill-mid-commit durability test.
- **M2 `spacedb-crdt`** — `CrdtDoc` over `yrs` (LWW-Register, PN-Counter, Y.Text,
  OR-Set), fuzzed convergence proof, encrypted persistence, reactive queries.
- **M3 `spacedb-replica`** — anti-entropy sync over a `Transport` seam,
  partition-heal with no lost writes, honest freshness (`Live`/`Stale`/`Unsynced`/
  `Partitioned`).
- **M4 `spacedb-durability`** — k-of-n erasure shards, anti-affinity placement,
  health, and a deterministic reconstruct→re-encode→re-place repair loop.
- **M5 `spacedb-access`** — P-256 mID identities, signed/scoped/expiring/revocable
  capabilities, narrowing delegation chains, a hash-chained audit log,
  human-vs-AI policy.
- **M6 `spacedb-query` + `spacedb-vector`** — deterministic, fuel/memory-bounded
  WASM compute-to-data with corroborated attestation; a partition-aware map-reduce
  planner with honest coverage; an on-node vector index (query in, top-k out,
  corpus never leaves), capability-gated.
- **M7 `spacedb-consistency`** — per-field tiers: convergent (CRDT), causal+
  (session read-your-writes / monotonic reads), and strong (a quorum that fails
  safe under partition — `Unavailable`, never a divergent commit). Every op
  reports the level it achieved.
- **M8 `spacedb-meter` + `spacedb-sdk` + `spacedb-console`** — deterministic
  metering (storage × time × replica_count, compute fuel, bilaterally-corroborated
  transit), rate-card pricing + agent budgets, a `Settlement` seam (a host plugs
  in `UsageClaim → Maestro → EarningRecord → Iron Bank`); the developer SDK above;
  and the operator console.

## Performance (0.7.3)

0.7.3 moves spacedb-store to rusty_zstd 0.4.2, whose new
`compress_into_with` appends a frame to a caller's buffer. A row is now
compressed into one buffer per thread and copied into the row only when the
compressed form is shorter; before, every row compressed allocated (and freed)
a fresh buffer sized to the compression bound, whether it was kept or not.
Same harness and drivers as 0.7.2, outputs unchanged:

| driver | 0.7.2 | 0.7.3 | change |
|---|---:|---:|---:|
| `ir_redb` (sealed rows on redb) | 330.43M | 329.51M | -0.28% |
| `ir_store` (sealed rows, in-memory engine) | 191.17M | 190.99M | -0.09% |
| `ir_crdt` (every CRDT type + sealed persistence) | 130.78M | 130.83M | +0.04% |
| the other twelve | | | within 0.02% |
| **all fifteen** | **2,598.99M** | **2,597.94M** | **-0.04%** |

The stored bytes are identical to 0.7.2's (rusty_zstd 0.4.2 emits exactly the
frames 0.4.1 did), so rows written by either version read in the other. No
API change; `cargo semver-checks` against 0.7.2 passes. rusty_zstd 0.4.2's
optional `rusty-alloc` feature goes through rusty_alloc_default 0.1.5, which
pins `rusty_alloc-api` 2.4.0, the same release spacedb-sdk pins; SpaceDB does
not enable that feature.

## Performance (0.7.2)

0.7.2 is a second instruction-count pass, over fifteen deterministic drivers
(`examples/ir_*.rs`; three were added for this round). Every change was
measured alone, kept only if its count fell with the driver's outputs
unchanged, and reverted with its number recorded otherwise. The baseline is
0.7.1 re-measured on the same harness, which is why `ir_store` starts lower
than in the 0.7.0 table below: rusty_zstd 0.4.1 (0.7.1) halved its decode cost.

| driver | 0.7.1 | 0.7.2 | change |
|---|---:|---:|---:|
| `ir_txrange` (range scans inside a write transaction) | 11.6M | 7.4M | **-36.1%** |
| `ir_console` (operator rollups and render) | 68.2M | 45.6M | **-33.1%** |
| `ir_vector` (1,500 x 64-d, three metrics, top-10) | 33.1M | 23.3M | **-29.8%** |
| `ir_query` (WASM map-reduce + host-call function) | 33.4M | 27.4M | **-18.0%** |
| `ir_ops` (CRDT mutations, one transaction each) | 33.9M | 29.7M | -12.7% |
| `ir_crdt` (every CRDT type + sealed persistence) | 146.3M | 130.8M | -10.6% |
| `ir_session` (two replicas syncing) | 11.3M | 10.2M | -9.6% |
| `ir_sdk` (authorize + write + read, 330 ops) | 12.2M | 11.1M | -9.0% |
| `ir_batch` (the `ir_ops` work in batches) | 15.5M | 14.2M | -8.4% |
| `ir_sim` (40-replica gossip, causal, quorum, churn) | 1,665.1M | 1,635.1M | -1.8% |
| `ir_claims` (usage claims through the SDK) | 14.4M | 14.2M | -1.6% |
| `ir_store` (sealed puts, gets, ranges; in-memory engine) | 194.1M | 191.2M | -1.5% |
| `ir_redb` (the same work on redb) | 335.0M | 330.4M | -1.4% |
| `ir_access` (authorize, delegation chains, audit log) | 95.6M | 95.3M | -0.3% |
| `ir_durability` (erasure encode + reconstruct) | 33.1M | 33.1M | 0.0% |
| **all fifteen** | **2,702.8M** | **2,599.0M** | **-3.8%** |

Where it came from:

- **Work no one used.** CRDT mutations no longer run a yrs cleanup observer
  that cloned three structures per transaction; each mutation finishes its own
  transaction, and one known to insert skips the state-vector comparison.
  Counter keys and values are written without the formatter.
- **Work done once.** `CrdtDoc::batch` runs many mutations in one transaction
  (the batch driver does the ops driver's work in under half the
  instructions). A redb transaction opens each table once, not once per row.
  A query hashes its snapshot and each module once, and the function linker is
  built once per runtime. Counter reads, set membership and
  `encode_update_since` answers are remembered until the document changes.
- **Borrowing instead of copying.** Range scans lend rows to a visitor
  (`Readable::range_with`), presence checks no longer copy the row
  (`Readable::contains_raw`), and collections take borrowed keys (`put_by`,
  `contains_key_by`, `get_with_by`).
- **Cheaper hashing and interning.** Short internal keys hash with FNV or an
  identity hasher instead of SipHash; the console interns names and builds
  its ordered maps once.
- **Vector kernels.** Dot product and L2 run on eight independent
  accumulators, so the compiler vectorizes them.

Behaviour that changed:

- Vector scores are summed in eight lanes, so an `f32` score can differ from
  0.7.1 in its last bits. Rankings in the test corpus are identical; two
  candidates within that rounding of each other could swap places.
- New rows are written without zstd's content checksum, because the AEAD tag
  already authenticates them. Earlier versions read these rows unchanged, and
  0.7.2 still reads rows written with the checksum.
- A redb transaction keeps each table open from first use until it ends.

New, all additive: `CrdtDoc::batch` with `Batch`, `CrdtDoc::state_vector_is`
and `state_vector_into`, `Readable::contains_raw` and `range_with` (with
defaults), `Collection::put_by` / `contains_key_by` / `get_with_by`,
`KeyDirectory::generation` (default `None`),
`HeldCapability::authorize_collection`, `Capability::covers_collection`,
`PinnedRecords`, `FunctionCtx::with_pinned`, `FunctionRuntime::run_prehashed`.
`cargo semver-checks` against 0.7.1 passes on all twelve crates.

Dependencies: spacedb-sdk now pins `rusty_alloc-api` 2.4.0 (from 2.2.6), the
latest release; spacedb-store adds `self_cell` 1 so a redb transaction can
hold its open tables without unsafe code. Further wins measured inside yrs,
primeorder and rusty_zstd are not in this release or in these numbers.

## Performance (0.7.0)

0.7.0 is a performance release: the same results, measured on **exact
instruction counts** (callgrind `Ir`) rather than a clock. Each crate has a
deterministic driver - `examples/ir_*.rs` - that does fixed work and prints
values that change if the work changes. A change was kept only if its count
went down and those values stayed identical; refuted changes were reverted
with their number recorded.

Instructions per driver, 0.6.0 -> 0.7.0, release build:

| driver | 0.6.0 | 0.7.0 | change |
|---|---:|---:|---:|
| `ir_sdk` (authorize + write + read, 330 ops) | 1,136.6M | 12.2M | **-98.9%** |
| `ir_access` (authorize, delegation chains, audit log) | 1,362.5M | 95.6M | **-93.0%** |
| `ir_query` (WASM map-reduce + host-call function) | 280.9M | 33.4M | **-88.1%** |
| `ir_vector` (1,500 x 64-d, three metrics, top-10) | 204.2M | 33.1M | **-83.8%** |
| `ir_crdt` (every CRDT type + sealed persistence) | 199.8M | 151.1M | -24.4% |
| `ir_sim` (40-replica gossip, causal, quorum, churn) | 2,060.5M | 1,663.3M | -19.3% |
| `ir_durability` (erasure encode + reconstruct) | 40.6M | 33.1M | -18.6% |
| `ir_store` (sealed puts, gets, ranges) | 416.9M | 356.1M | -14.6% |
| **all eight** | **5,702M** | **2,378M** | **-58.3%** |

The operator console's driver (`ir_console`, added in this release) went
91.0M -> 82.9M (-8.8%) before the build change below.

Where it came from, largest first:

- **Work done once instead of per call.** Compiled WASM modules are cached by
  workload hash; an SDK session verifies its capability's signature once and
  re-checks only if the issuer's published key changes; signatures already
  verified are remembered process-wide (bounded; only successes; keyed by a
  BLAKE3 of key, message and signature); erasure coders are built once per
  geometry; a range scan builds one cipher.
- **Algorithmic.** Vector insert is O(1) by id (was a linear scan) and search
  selects the top k instead of sorting everything.
- **Work no one used.** Remote CRDT updates are no longer re-encoded for an
  update log that already holds them; documents that never relay raw updates
  can opt out of the log (`CrdtDoc::new_unlogged`).
- **Copies.** Rows are sealed and opened in place, byte strings serialize as
  one copy (`Bytes`), and buffers move instead of being cloned across the
  engine, shard-store and transport seams. Every copy site in the source is
  listed with its verdict in the mono's `docs/plans/spacedb-copy-inventory.md`.
- **Build.** In the MATA monorepo the SpaceDB crates now build at
  `opt-level = 3` for native release (the web build stays size-tuned).
  Embedders choose their own profile; the table above includes it.

Behaviour that changed:

- `CrdtStore::contains` is a presence check: it no longer needs the vault
  unlocked or decrypts the document.
- Signature verification is cached per process (see above). A log walk does
  not re-verify a signature the same process just produced in
  `AuditLog::record`; a tampered entry is still verified in full.

New, all additive: `Bytes`, `Collection::contains_key`,
`Collection<K, Bytes>::get_with`, `WriteTx::put_raw_owned`,
`HeldCapability`, `KeyDirectory::published_key_is`, `CrdtDoc::new_unlogged`,
`CrdtDoc::caught_up_state_vector`, `SyncMessage::decode_ref` / `into_frame`,
`Transport::send_owned`, `ShardStore::put_owned` / `len_of`. New trait methods
have default implementations, so existing implementors compile unchanged.

Counts are exact run to run once randomness is pinned: the drivers seed the
CRDT engine's id generator, and a measurement run answers `getrandom`
deterministically (an `LD_PRELOAD` shim) so hash seeds, nonces and test keys
repeat. Reproduce (Linux, valgrind):

```bash
cargo build --release --example ir_access -p spacedb-access
valgrind --tool=callgrind target/release/examples/ir_access
```

## Building & testing

The workspace defaults to `wasm32`; the SpaceDB crates are native. Test them on
your host target, e.g.:

```bash
cargo test -p spacedb-sdk --target x86_64-pc-windows-msvc      # the SDK end-to-end
cargo run  -p spacedb-console --example snapshot --target x86_64-pc-windows-msvc
```

## Open-core boundary

No `spacedb-*` crate depends on any proprietary MATA crate (`mata-*`, `maestro-*`,
`disco*`, `iron-bank*`). The dependency arrow only ever points **MATA → SpaceDB**, and a
CI test enforces it — so this folder extracts into its own repository as a mechanical
`git mv`, and the published crates pull in nothing closed.

## License

Dual-licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you shall be dual-licensed as above, without any
additional terms or conditions.
