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
