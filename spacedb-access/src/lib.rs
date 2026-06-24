#![forbid(unsafe_code)]
//! # spacedb-access — SpaceDB Layer 5 (identity & access)
//!
//! The consent layer, and the AI-age differentiator: **inaccessible by default,
//! accessible by mID-gated consent**. Every read / write / compute is authorized
//! by a signed, scoped, expiring, (S2) revocable [`Capability`] issued by an
//! owner's identity to a bearer — a human or an AI agent with its *own* identity.
//!
//! M5-S1 ships the core: [`Identity`] (ECDSA P-256 / ES256), the [`Capability`] +
//! [`SignedCapability`] model, the [`KeyDirectory`] seam (DID → published key),
//! and [`authorize`] — the single chokepoint enforcing signature · bearer · scope
//! · ops · expiry. Revocation + delegation (S2) and the audit log + human-vs-AI
//! policy (S3) build on this.
//!
//! Open-core (MIT): no MATA dependency. Identities are P-256 keys behind the
//! `KeyDirectory` seam; MATA resolves `did:mata` via IAMHUMAN, a self-hoster uses
//! [`MemKeyDirectory`]. ES256 matches mID, so MATA's real mIDs verify identically.

mod error;
pub use error::{AccessError, AccessResult};

mod identity;
pub use identity::{Did, Identity};

mod directory;
pub use directory::{KeyDirectory, MemKeyDirectory};

mod capability;
pub use capability::{Capability, Ops, Scope, SignedCapability};

mod revocation;
pub use revocation::RevocationSet;

mod chain;
pub use chain::{delegate, CapabilityChain};

mod authorize;
pub use authorize::{
    authorize, authorize_chain, AccessRequest, Decision, DelegationError, DenyReason,
};

mod policy;
pub use policy::{gate, AccessPolicy};

mod audit;
pub use audit::{AuditDecision, AuditEntry, AuditError, AuditLog, AuditResult};
