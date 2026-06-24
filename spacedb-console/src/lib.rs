#![forbid(unsafe_code)]
//! # spacedb-console — the operator's read-model
//!
//! The substance of an operations console is not pixels — it is the logic that
//! turns raw fleet / access / audit / settlement [`Observations`] into the four
//! boards a SpaceDB business is run from:
//!
//! 1. **Fleet Health** ([`FleetHealth`]) — durability + availability: under-replicated,
//!    at-risk and lost shards, quorum loss, lag; one Green/Amber/Red rollup.
//! 2. **Access & Audit** ([`AccessOverview`]) — human vs AI-agent capabilities,
//!    expirations, revocations, per-agent activity, denials.
//! 3. **Economics** ([`Economics`]) — revenue, spend per customer, per-rail split,
//!    agent budget burn-down, unsettled claims.
//! 4. **Alerts** ([`Alert`]) — the page-me surface, severity-sorted.
//!
//! [`Dashboard::assemble`] computes all four from one snapshot; [`Dashboard::render_text`]
//! prints them. A Dioxus/WASM shell binds the same fields to a UI:
//!
//! ```ignore
//! // in the Dioxus shell (compiles to wasm32, not part of the tested core):
//! let dash = Dashboard::assemble(&observations, &Config::at(now));
//! rsx! {
//!     HealthBadge { status: dash.health.status }
//!     for alert in dash.alerts { AlertRow { alert } }
//!     EconomicsPanel { economics: dash.economics }
//! }
//! ```
//!
//! Open-core (MIT). Fed by observation DTOs, so it stays decoupled and testable.

mod observe;
pub use observe::{
    AgentBudgetObs, AuditObs, CapabilityObs, HomeObs, LagObs, SettledObs, ShardObs, StrongObs,
};

mod fleet;
pub use fleet::{assess_fleet, FleetHealth, HealthStatus};

mod alerts;
pub use alerts::{derive_alerts, Alert, AlertKind, AlertThresholds, Severity};

mod economics;
pub use economics::{rollup_economics, BudgetStatus, Economics, RailBreakdown};

mod access;
pub use access::{rollup_access, AccessOverview, AgentActivity};

mod dashboard;
pub use dashboard::{Config, Dashboard, Observations};

// Re-export the shared economics vocabulary.
pub use spacedb_meter::Resource;
