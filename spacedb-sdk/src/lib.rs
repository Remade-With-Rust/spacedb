#![forbid(unsafe_code)]
//! # spacedb-sdk — the developer's whole world
//!
//! One surface over the entire SpaceDB stack. You [`open`](Database::open) an
//! offline-first local replica, [`define`](Database::define) a [`Schema`] where
//! each field declares its [`CrdtType`] and consistency [`Tier`], and run ops that
//! are **mID-authorized**, **budget-bounded**, and **honest** — every write and
//! read returns the [`Outcome`] it actually achieved (`Local`, `Committed{tier}`,
//! `Stale{lag}`, `Unavailable{reason}`). Strong-tier fields go through a quorum
//! that fails safe under partition; reactive [`Watcher`]s and CRDT
//! [`export`](Database::export)/[`import`](Database::import) sync round it out.
//!
//! ```no_run
//! use spacedb_sdk::{Database, Schema, CrdtType, Tier, Identity};
//!
//! let owner = Identity::generate("did:mata:owner").unwrap();
//! let mut db = Database::open(Identity::generate("did:mata:home-1").unwrap());
//! db.register_identity(&owner).unwrap();
//! db.define(
//!     Schema::new("profile")
//!         .field("bio", CrdtType::Text, Tier::Convergent)
//!         .field("username", CrdtType::Register, Tier::Strong),
//! );
//! ```
//!
//! Open-core (MIT). Composes `spacedb-crdt`, `-access`, `-consistency`, `-meter`.


/// The pure-Rust global allocator, installed process-wide.
///
/// Present with the **default** `rusty-alloc` feature. SpaceDB is a distributed
/// database whose replicas routinely run on machines their operator does not
/// control, so the allocator is part of the failure surface, not an
/// implementation detail: a double free **aborts** instead of corrupting the
/// heap, and the tree carries no C allocator. An unconfigured node should be the
/// hardened one, so this is opt-**out**:
///
/// ```toml
/// spacedb-sdk = { version = "0.5", default-features = false }  # bring your own
/// spacedb-sdk = { version = "0.5", features = ["secure"] }     # + guard pages
/// ```
///
/// Disabling it removes `rusty_alloc` from the dependency graph entirely, not
/// merely from a `cfg`, leaving you free to declare your own.
///
/// ## If you are writing a LIBRARY that depends on this crate
///
/// Set `default-features = false`. A program may contain exactly **one**
/// `#[global_allocator]`, and Cargo features are **additive across the whole
/// dependency graph** — a library that pulled this crate with defaults on would
/// impose this allocator on every application downstream, and any application
/// that had already chosen its own would fail to build with
/// `the #[global_allocator] in this crate conflicts with global allocator in:
/// spacedb_sdk`, which it could not fix from its own manifest.
#[cfg(feature = "rusty-alloc")]
#[global_allocator]
static GLOBAL: rusty_alloc_api::RustyAlloc = rusty_alloc_api::RustyAlloc;

/// Whether this build installed `rusty_alloc` as the global allocator.
///
/// Worth logging at node startup: the allocator is a deployment property, and a
/// property you cannot observe is one you cannot verify.
pub const fn rusty_alloc_enabled() -> bool {
    cfg!(feature = "rusty-alloc")
}

/// Whether the hardened `secure` profile (guard pages, encrypted free lists) is
/// compiled in.
pub const fn secure_allocator_enabled() -> bool {
    cfg!(feature = "secure")
}

mod schema;
pub use schema::{CrdtType, FieldSpec, Schema};

mod error;
pub use error::{SdkError, SdkResult};

mod session;
pub use session::Session;

mod db;
pub use db::Database;

// Re-export the stack types a developer composes with, so one `use` line suffices.
pub use spacedb_access::{
    Capability, Did, Identity, MemKeyDirectory, Ops, RevocationSet, Scope, SignedCapability,
};
pub use spacedb_consistency::{Outcome, RejectReason, StrongResult, Tier, UnavailableReason};
pub use spacedb_crdt::Watcher;
pub use spacedb_meter::Budget;

/// Compiles the README's examples as doctests, so the documented API can never
/// drift from the real one. Not part of the public API, and not rendered into
/// the crate docs — it exists only under `cargo test --doc`.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
