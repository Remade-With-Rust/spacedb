# spacedb-sdk

[![crates.io](https://img.shields.io/crates/v/spacedb-sdk?logo=rust)](https://crates.io/crates/spacedb-sdk)
[![docs.rs](https://img.shields.io/docsrs/spacedb-sdk?logo=docsdotrs)](https://docs.rs/spacedb-sdk)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-MIT)
[![Remade With Rust](https://img.shields.io/badge/Remade%20With-Rust-000?logo=rust&logoColor=fff)](https://github.com/Remade-With-Rust)
[![By Mata Network](https://img.shields.io/badge/by-Mata%20Network-5b2be0)](https://www.mata.network/)

**The developer's whole world — one surface over the entire SpaceDB stack.**

This is the crate you add. It composes [`spacedb-crdt`](https://github.com/Remade-With-Rust/spacedb/tree/HEAD/spacedb-crdt),
[`spacedb-access`](https://github.com/Remade-With-Rust/spacedb/tree/HEAD/spacedb-access),
[`spacedb-consistency`](https://github.com/Remade-With-Rust/spacedb/tree/HEAD/spacedb-consistency) and
[`spacedb-meter`](https://github.com/Remade-With-Rust/spacedb/tree/HEAD/spacedb-meter) into a single `Database`: offline-first,
schema-declared per field, mID-authorized, budget-bounded, and honest about what
every operation actually achieved.

Part of [SpaceDB](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/README.md). Dual-licensed **MIT OR Apache-2.0**.

```toml
[dependencies]
spacedb-sdk = "0.5"
```

## Memory allocation (`rusty_alloc`) — on by default

**`spacedb-sdk` installs [`rusty_alloc`](https://github.com/remade-with-rust/rusty_alloc)
as the process-wide allocator by default, and we highly recommend leaving it on
for any distributed deployment.**

SpaceDB is a *distributed* database: replicas routinely run on machines their
operator does not control. On that hardware the allocator is part of the failure
surface, not an implementation detail.

| property | why it matters on a node you don't own |
|---|---|
| **Double free aborts** instead of corrupting the heap (~0.4% overhead) | A memory bug becomes a crash you can see rather than silent divergence you cannot trust. A replica that corrupts its own heap is a replica that lies to the mesh. |
| **Pure Rust — no C allocator in the tree** | The allocator under a database holding other people's encrypted data is code you can audit; it is also the last major C dependency `std` otherwise drags in. |
| **`secure` feature** — guard pages + encrypted free lists | ~4–7% instructions (measured). Worth it for nodes taking untrusted input. |
| **One allocator on every target** | Linux, macOS, Windows and `wasm32` (no emscripten), so a node behaves the same wherever it is placed. |

An unconfigured node should be the hardened one, so this is opt-**out**:

```toml
# Bring your own allocator (jemalloc, mimalloc, the system one, …)
spacedb-sdk = { version = "0.5", default-features = false }

# Or the hardened node profile
spacedb-sdk = { version = "0.5", features = ["secure"] }
```

Disabling it removes `rusty_alloc` from the dependency graph entirely — not
merely from a `cfg` — leaving you free to declare your own `#[global_allocator]`.
The API is identical either way. Check what a build actually got:

```rust
println!("hardened allocator: {}", spacedb_sdk::rusty_alloc_enabled());
```

### Writing a library? Opt out.

A program may contain exactly **one** `#[global_allocator]`, and Cargo features
are **additive across the entire dependency graph**. If a *library* depended on
`spacedb-sdk` with default features, every application using it would inherit
this allocator — and any application that had already chosen its own would fail
to build with:

```text
error: the `#[global_allocator]` in this crate conflicts with global allocator in: spacedb_sdk
```

…which it could not fix from its own manifest. **Applications** decide the
allocator; **libraries** should stay out of that decision.

## The whole model in one page

```rust
use spacedb_sdk::{
    Capability, CrdtType, Database, Identity, Ops, Outcome, Schema,
    Scope, SignedCapability, StrongResult, Tier,
};

// 1. Open an offline-first local replica for this device.
let mut db = Database::open(Identity::generate("did:mata:home-1")?);

// 2. Declare a schema — each field picks its CRDT type AND its consistency tier.
db.define(
    Schema::new("profile")
        .field("bio",          CrdtType::Text,     Tier::Convergent)
        .field("display_name", CrdtType::Register, Tier::Convergent)
        .field("cursor",       CrdtType::Register, Tier::Causal)
        .field("visits",       CrdtType::Counter,  Tier::Convergent)
        .field("username",     CrdtType::Register, Tier::Strong),
);

// 3. The owner grants a capability — to a person or an AI agent.
let owner = Identity::generate("did:mata:owner")?;
db.register_identity(&owner)?;
db.set_clock(1_700_000_000);

let cap = Capability::grant(
        owner.did().clone(),
        "did:agent:assistant",
        Scope::Collection("profile".into()),
        Ops::READ | Ops::WRITE,
    )?
    .with_expiry(1_702_592_000)
    .with_budget(1_000_000);          // micro-$MATA it may spend
let mut session = db.session(SignedCapability::sign(cap, &owner)?);

// 4. Write offline. Every op returns the consistency it ACTUALLY achieved.
let outcome = db.put_register(&mut session, "profile", "display_name", "Ada")?;
assert_eq!(outcome, Outcome::Local);  // durable here, converging outward
db.increment(&mut session, "profile", "visits", 1)?;
db.append_text(&mut session, "profile", "bio", "building on SpaceDB")?;

// 5. Read it back — honest about freshness.
let (name, read) = db.read_register(&mut session, "profile", "display_name")?;

// 6. Strong tier when you mean it: globally unique, or it cleanly refuses.
match db.claim_unique(&mut session, "profile", "username", "ada")? {
    StrongResult::Committed      => println!("username is yours"),
    StrongResult::Rejected(_)    => println!("already taken"),
    StrongResult::Unavailable(_) => println!("no quorum right now — try later"),
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

**open → schema → grant → write/read with honest state → strong when you mean
it.** No connection string, no server, no network required.

## Sync two replicas (still no server)

```rust,ignore
let bytes = laptop.export("profile");     // CRDT state, content-addressed
phone.import("profile", &bytes)?;         // merges; conflicts resolve by CRDT rules
```

Export the collection *after* it has been written — exporting one that has never
taken a write produces an empty update that `import` rejects.

## React to change

```rust,ignore
let watcher = db.watch("profile");
// ... after any local or merged write:
if watcher.drain_changed() { /* re-render */ }
```

## What the SDK enforces for you

- **Schema.** `require_field` rejects an op whose field wasn't declared, or was
  declared as a different CRDT type — `claim_unique` on a non-`Strong` field is
  an error, not a silent downgrade.
- **Authorization.** Every op runs through the `spacedb-access` chokepoint with
  the session's capability. No capability, no write.
- **Budget.** Each mutating op is charged at `write_cost()`; a session that
  exhausts its budget stops rather than overdrawing. `session.budget_remaining()`
  reports it.
- **Honesty.** `Outcome` (`Local` / `Committed{tier}` / `Stale{lag}` /
  `Unavailable{reason}`) and `StrongResult` come back from every op. Nothing is
  reported as durable that isn't.
- **Revocation.** `db.revoke(capability_id)` cuts off the bearer and everything
  delegated beneath it.

`quorum_partition` / `quorum_heal` let a test take strong-tier members offline
and prove the group fails safe instead of splitting.

## Testing

The workspace defaults to `wasm32`; this crate is native. Test on your host
triple:

```bash
cargo test -p spacedb-sdk --target aarch64-apple-darwin   # or your host triple
```

Suite: `sdk.rs` — the end-to-end developer path above.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-MIT) and
[LICENSE-APACHE](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-APACHE).
