# SpaceDB

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

---

## Quick start

Add the SDK — it's the one crate you need; it composes the whole stack:

```toml
[dependencies]
spacedb-sdk = "0.1"
```

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

## The layers

One crate per layer; `spacedb-sdk` ties them together. Each defines a **seam** an
operator implements for the hosted product — the dependency arrow is
**MATA → SpaceDB**, never the reverse.

| Crate | Layer | What it gives you | Seam an operator fills |
|---|---|---|---|
| `spacedb-store` | L0 storage | encrypted KV, typed tables | `KvEngine`, `KeyProvider` (vault) |
| `spacedb-crdt` | L1 data | convergent docs, reactive queries | — |
| `spacedb-replica` | L2 sync | anti-entropy sync, honest freshness | `Transport` (iroh/relay) |
| `spacedb-durability` | L2 mesh | erasure shards, placement, self-repair | `ShardStore`, placement |
| `spacedb-access` | L5 access | mID capabilities, delegation, audit | `KeyDirectory` (mID/IAMHUMAN) |
| `spacedb-query` | L4 compute | deterministic compute-to-data + attest | redundant placement |
| `spacedb-vector` | L4 RAG | on-node top-k; corpus never leaves | — |
| `spacedb-consistency` | L3 tiers | convergent / causal+ / strong | strong-tier placement |
| `spacedb-meter` | L6 economics | metering, pricing, budgets | `Settlement` → Iron Bank |
| `spacedb-sdk` | — | **the developer surface** (above) | — |
| `spacedb-console` | — | operator dashboard read-model | live observation adapters |

---

## Status — Phase 1 complete

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
