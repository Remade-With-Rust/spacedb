#![forbid(unsafe_code)]
//! # spacedb-meter — SpaceDB Layer 6 (metering & settlement)
//!
//! SpaceDB **measures**; it does not price or pay. This crate computes the three
//! resource amounts deterministically — storage ([`Usage::storage`], bytes × time ×
//! replica_count), compute ([`Usage::compute`], fuel + invocations), and transit
//! ([`Usage::transit`], bilaterally-corroborated served bytes) — accumulates them
//! in a [`MeterLedger`], and drains them into amounts-only [`UsageClaim`]s.
//!
//! Pricing ([`RateCard`]), the agent spend cap ([`Budget`]), and pre-deploy cost
//! estimates are open-core too. **Settlement is the seam**: the bundled
//! [`LocalSettlement`] is enough for a self-hoster, while a host implements
//! [`Settlement`] over its own money plane — for MATA that is the existing
//! `UsageClaim → Maestro counter-sign → EarningRecord → Iron Bank` loop — so
//! SpaceDB usage settles through it without ever depending on it.
//!
//! Open-core (MIT). No `spacedb-*` / `mata-*` dependencies.

mod error;
pub use error::{MeterError, MeterResult};

mod usage;
pub use usage::{ProofKind, ProofRef, Resource, Usage};

mod claim;
pub use claim::UsageClaim;

mod ratecard;
pub use ratecard::RateCard;

mod budget;
pub use budget::Budget;

mod ledger;
pub use ledger::{MeterLedger, Totals};

mod settle;
pub use settle::{LocalSettlement, Settled, Settlement};
