//! The [`UsageClaim`] — the settlement message SpaceDB emits.
//!
//! It carries the *amount* (never a price), who did the work, and who the earnings
//! settle to. Its shape mirrors `maestro_edge::UsageClaim`, so a host's adapter
//! (e.g. MATA's) is a thin field translation before it enters the existing
//! `UsageClaim → Maestro counter-sign → EarningRecord → Iron Bank` pipeline.

use serde::{Deserialize, Serialize};

use crate::error::MeterError;
use crate::usage::{ProofRef, Resource, Usage};

/// A unit of metered work, ready for settlement. Amounts only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageClaim {
    pub version: u16,
    /// Deterministic dedup key (`node:settles_to:class:period_end`).
    pub claim_id: String,
    /// The node that did the work (its own identity).
    pub node_did: String,
    /// The mID earnings settle to / the customer billed.
    pub settles_to_did: String,
    pub usage: Usage,
    /// Unix seconds bounding the measured period.
    pub period_start: u64,
    pub period_end: u64,
    /// Optional proof backing the amount.
    pub proof: Option<ProofRef>,
}

impl UsageClaim {
    pub const VERSION: u16 = 1;

    pub fn new(
        node_did: impl Into<String>,
        settles_to_did: impl Into<String>,
        usage: Usage,
        period_start: u64,
        period_end: u64,
    ) -> Self {
        let node_did = node_did.into();
        let settles_to_did = settles_to_did.into();
        let claim_id = format!(
            "{node_did}:{settles_to_did}:{}:{period_end}",
            usage.resource().tag()
        );
        Self {
            version: Self::VERSION,
            claim_id,
            node_did,
            settles_to_did,
            usage,
            period_start,
            period_end,
            proof: None,
        }
    }

    /// Attach a backing proof.
    pub fn with_proof(mut self, proof: ProofRef) -> Self {
        self.proof = Some(proof);
        self
    }

    pub fn resource(&self) -> Resource {
        self.usage.resource()
    }

    pub fn encode(&self) -> Result<Vec<u8>, MeterError> {
        postcard::to_allocvec(self).map_err(|e| MeterError::Codec(e.to_string()))
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, MeterError> {
        postcard::from_bytes(bytes).map_err(|e| MeterError::Codec(e.to_string()))
    }
}
