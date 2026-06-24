# Security Policy

SpaceDB underpins encrypted, capability-gated data — its cryptographic and
access boundaries are load-bearing. We take security reports seriously and want
to hear about them **privately, first**.

## Reporting a vulnerability

**Please do not open a public issue for a security vulnerability.**

Report it privately through **GitHub's private vulnerability reporting** —
the *Security → Report a vulnerability* button on this repository. If that is
unavailable, email the maintainers at **security@mata.network**.

Please include:
- a description of the issue and the impact you believe it has,
- the crate(s) and version(s) affected,
- steps to reproduce (a failing test or proof-of-concept is ideal).

We aim to acknowledge within **3 business days** and to keep you updated as we
investigate. Once a fix is ready we'll coordinate disclosure with you and credit
you, unless you'd prefer to remain anonymous.

## Supported versions

SpaceDB is pre-1.0; security fixes land on the latest `0.x` release. Pin a
version and watch the repository for releases.

## Scope

**In scope** — the `spacedb-*` crates in this repository:
- the AEAD value boundary and key wrapping (`spacedb-store`),
- CRDT convergence and data integrity (`spacedb-crdt`, `spacedb-replica`),
- capability / authorization enforcement and delegation (`spacedb-access`),
- the consistency tiers and their fail-safe guarantees (`spacedb-consistency`).

**Out of scope** — the proprietary MATA services that *embed* SpaceDB (the mesh
of homes, settlement, the hosted control plane). Report those through MATA's
product security channel, not here.
