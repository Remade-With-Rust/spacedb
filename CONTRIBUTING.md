# Contributing to SpaceDB

Thanks for your interest. SpaceDB is the open core of the MATA stack — a
local-first, CRDT-native, mesh-replicated database — dual-licensed
**MIT OR Apache-2.0**.

## Build & test

```bash
cargo build
cargo test                 # the crates are native + wasm-aware; test on your host target
cargo test -p spacedb-sdk  # the SDK end-to-end
```

`cargo fmt` and `cargo clippy` should be clean before you open a PR.

## The open-core boundary — the one hard rule

SpaceDB depends on **nothing proprietary**. The dependency arrow only ever points
**MATA → SpaceDB**, never the reverse — that one-directional boundary is what lets
SpaceDB live in the open and publish to crates.io without dragging closed code
along. Concretely:

- A `spacedb-*` crate must **never** depend on a closed MATA crate (`mata-*`,
  `maestro-*`, `disco*`, `iron-bank*`, …). If you need something shared, put it in
  a `spacedb-*` crate, or invert the dependency.
- Each third-party dependency is declared inline (with a version) in each crate;
  the workspace only shares the `spacedb-*` crates with one another.

A CI check enforces this on every change, so a PR that crosses the boundary will
fail before it can merge.

## Pull requests

- Keep PRs focused, and include tests for any behavior change.
- Convergence- and crypto-adjacent changes need a test that proves the invariant
  (e.g. fuzzed CRDT convergence, a kill-mid-commit durability case).
- By submitting a contribution you agree it is dual-licensed MIT OR Apache-2.0,
  matching the project, with no additional terms.

## Security issues

Please **don't** file security vulnerabilities as public issues — see
[SECURITY.md](SECURITY.md) for private reporting.
