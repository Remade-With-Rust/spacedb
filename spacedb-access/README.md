# spacedb-access

[![crates.io](https://img.shields.io/crates/v/spacedb-access?logo=rust)](https://crates.io/crates/spacedb-access)
[![docs.rs](https://img.shields.io/docsrs/spacedb-access?logo=docsdotrs)](https://docs.rs/spacedb-access)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-MIT)
[![Remade With Rust](https://img.shields.io/badge/Remade%20With-Rust-000?logo=rust&logoColor=fff)](https://github.com/Remade-With-Rust)
[![By Mata Network](https://img.shields.io/badge/by-Mata%20Network-5b2be0)](https://www.mata.network/)

**SpaceDB Layer 5 — identity & access.**

The consent layer, and the AI-age differentiator: **inaccessible by default,
accessible by mID-gated consent.** Every read / write / compute is authorized by
a signed, scoped, expiring, revocable capability issued by an owner's identity to
a bearer — a human, or an AI agent with its *own* identity.

Part of [SpaceDB](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/README.md). Dual-licensed **MIT OR Apache-2.0**.

```toml
[dependencies]
spacedb-access = "0.5"
```

## The model

```rust
use spacedb_access::{
    authorize, AccessRequest, Capability, Did, Identity, MemKeyDirectory,
    Ops, RevocationSet, Scope, SignedCapability,
};

let now = 1_700_000_000;
let owner = Identity::generate("did:mata:owner").unwrap();
let directory = MemKeyDirectory::new();
directory.publish(&owner).unwrap();
let agent = Did::from("did:agent:assistant");

// Grant an AI agent read+write on one collection, expiring, with a spend cap.
let cap = Capability::grant(
        owner.did().clone(),
        agent.clone(),
        Scope::Collection("profile".into()),
        Ops::READ | Ops::WRITE,
    ).unwrap()
    .with_expiry(1_702_592_000)
    .with_budget(1_000_000)          // micro-$MATA it may spend
    .with_delegation_depth(1);       // it may sub-grant once, never broader
let signed = SignedCapability::sign(cap, &owner).unwrap();

let scope = Scope::Document { collection: "profile".into(), doc_id: "me".into() };
let request = AccessRequest { bearer: &agent, scope: &scope, op: Ops::READ };
let decision = authorize(&signed, &request, &directory, now, &RevocationSet::new()).unwrap();
assert!(decision.is_allowed());
```

`authorize` is the **single chokepoint**. It enforces, in order: issuer key
resolves → signature verifies → not revoked → bearer matches → scope covers the
request → op is granted → not expired. Every rejection is a named `DenyReason`
(`UnknownIssuer`, `BadSignature`, `BearerMismatch`, `OutOfScope`, `OpNotGranted`,
`Expired`, `Revoked`, `Delegation(..)`, `EmptyChain`, `NoCapability`,
`NotAccountable`) — never a bare `false`.

## Scopes and ops

`Scope` is `Collection(name)` (covers every document in it), `Document
{ collection, doc_id }`, or `Function(name)` for compute. `Ops` is a bitset of
`READ` / `WRITE` / `COMPUTE`, with `contains` and `is_subset_of`.

## Delegation that can only narrow

`delegate(&parent_chain, sub_capability, &delegator)` appends a link to a
`CapabilityChain`; `authorize_chain` walks it. A link is rejected unless the
sub-grant's issuer is the link above it's bearer, the parent is delegable, the
depth decreases, and the scope and ops are a **subset** of the parent's. Scope
escalation is structurally impossible, not merely discouraged.

## Revocation

`RevocationSet::revoke(capability_id)` kills a capability and, through
`authorize_chain`, everything delegated beneath it.

## Human vs AI policy

`AccessPolicy` + `gate(..)` express the stance a deployment takes:
roster humans read freely; an AI agent with no grant is denied `NoCapability`;
`requiring_accountable_agents()` additionally requires an agent's chain to root
at an accountable roster member (`NotAccountable` otherwise).

## Tamper-evident audit

`AuditLog::record(..)` appends a hash-chained entry per decision (who, what
scope, which op, which capability, allow/deny) signed by the node identity;
`verify(node_public_key)` re-walks the chain and fails on any edit, reorder, or
deletion.

## Seams an operator fills

- **`KeyDirectory`** — DID → published key. `MemKeyDirectory` ships for
  self-hosting and tests; MATA resolves `did:mata` over IAMHUMAN.

Identities are ECDSA **P-256 / ES256**, the same primitive as mID — so MATA's
real mIDs verify here identically, with no MATA dependency in this crate.

## Testing

The workspace defaults to `wasm32`; this crate is native. Test on your host
triple:

```bash
cargo test -p spacedb-access --target aarch64-apple-darwin   # or your host triple
```

Suites: `authorize.rs`, `delegation.rs`, `revocation.rs`, `policy.rs`,
`audit.rs`, `consent_flow.rs`.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-MIT) and
[LICENSE-APACHE](https://github.com/Remade-With-Rust/spacedb/blob/HEAD/LICENSE-APACHE).
