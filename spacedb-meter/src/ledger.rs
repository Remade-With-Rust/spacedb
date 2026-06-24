//! The local meter — accumulate raw usage, then drain it into settlement claims.
//!
//! A node records every metered op as it happens; periodically it drains the
//! accumulated totals into one [`UsageClaim`] per `(customer, resource class)` to
//! hand to settlement. Draining clears the totals, so nothing is billed twice.

use std::collections::BTreeMap;

use crate::claim::UsageClaim;
use crate::usage::{Resource, Usage};

/// Accumulated amounts for one customer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub storage_byte_seconds: u128,
    pub compute_fuel: u64,
    pub compute_invocations: u64,
    pub transit_bytes: u64,
}

impl Totals {
    fn add(&mut self, usage: Usage) {
        match usage {
            Usage::Storage { byte_seconds } => self.storage_byte_seconds += byte_seconds,
            Usage::Compute { fuel, invocations } => {
                self.compute_fuel += fuel;
                self.compute_invocations += invocations;
            }
            Usage::Transit { bytes_served } => self.transit_bytes += bytes_served,
        }
    }

    fn is_empty(&self) -> bool {
        *self == Totals::default()
    }
}

/// Accumulates usage per customer mID until drained into claims.
#[derive(Clone, Debug, Default)]
pub struct MeterLedger {
    /// settles_to_did → accumulated totals. BTreeMap for deterministic drain order.
    totals: BTreeMap<String, Totals>,
}

impl MeterLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one metered op against the customer it settles to.
    pub fn record(&mut self, settles_to: impl Into<String>, usage: Usage) {
        self.totals.entry(settles_to.into()).or_default().add(usage);
    }

    /// The accumulated totals for a customer (zero if none).
    pub fn totals(&self, settles_to: &str) -> Totals {
        self.totals.get(settles_to).copied().unwrap_or_default()
    }

    /// Drain all accumulated usage into claims attributed to `node_did` over the
    /// period, then reset. One claim per non-zero `(customer, resource class)`.
    pub fn drain_claims(
        &mut self,
        node_did: &str,
        period_start: u64,
        period_end: u64,
    ) -> Vec<UsageClaim> {
        let mut claims = Vec::new();
        for (settles_to, totals) in std::mem::take(&mut self.totals) {
            if totals.is_empty() {
                continue;
            }
            let mut push = |usage: Usage| {
                claims.push(UsageClaim::new(
                    node_did,
                    &settles_to,
                    usage,
                    period_start,
                    period_end,
                ));
            };
            if totals.storage_byte_seconds > 0 {
                push(Usage::Storage {
                    byte_seconds: totals.storage_byte_seconds,
                });
            }
            if totals.compute_fuel > 0 || totals.compute_invocations > 0 {
                push(Usage::Compute {
                    fuel: totals.compute_fuel,
                    invocations: totals.compute_invocations,
                });
            }
            if totals.transit_bytes > 0 {
                push(Usage::Transit {
                    bytes_served: totals.transit_bytes,
                });
            }
        }
        claims
    }

    /// Whether any usage is pending.
    pub fn is_empty(&self) -> bool {
        self.totals.values().all(Totals::is_empty)
    }

    /// The resource classes currently pending for a customer.
    pub fn pending_classes(&self, settles_to: &str) -> Vec<Resource> {
        let t = self.totals(settles_to);
        let mut classes = Vec::new();
        if t.storage_byte_seconds > 0 {
            classes.push(Resource::Storage);
        }
        if t.compute_fuel > 0 || t.compute_invocations > 0 {
            classes.push(Resource::Compute);
        }
        if t.transit_bytes > 0 {
            classes.push(Resource::Transit);
        }
        classes
    }
}
