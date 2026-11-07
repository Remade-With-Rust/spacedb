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

/// Append `n` in decimal, as `Display` would print it.
fn push_decimal(out: &mut String, mut n: u64) {
    let mut digits = [0u8; 20];
    let mut at = digits.len();
    loop {
        at -= 1;
        digits[at] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    for &d in &digits[at..] {
        out.push(d as char);
    }
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
        // `node:settles_to:class:period_end`, joined into one exactly-sized
        // String: the same bytes `format!` produced, without the formatting
        // machinery (a few hundred instructions per piece) on every claim.
        let tag = usage.resource().tag();
        let mut claim_id =
            String::with_capacity(node_did.len() + settles_to_did.len() + tag.len() + 3 + 20);
        claim_id.push_str(&node_did);
        claim_id.push(':');
        claim_id.push_str(&settles_to_did);
        claim_id.push(':');
        claim_id.push_str(tag);
        claim_id.push(':');
        push_decimal(&mut claim_id, period_end);
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

#[cfg(test)]
mod claim_id_tests {
    use super::*;

    #[test]
    fn claim_id_matches_the_formatted_form() {
        for (node, cust, end) in [("n", "c", 0u64), ("home-007", "customer-3", 1_700_000_060), ("a:b", "", u64::MAX)] {
            let claim = UsageClaim::new(node, cust, Usage::compute(5, 1), 0, end);
            assert_eq!(claim.claim_id, format!("{node}:{cust}:compute:{end}"));
        }
    }
}
