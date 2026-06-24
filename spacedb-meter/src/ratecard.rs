//! Pricing — the one place amounts become money, kept swappable.
//!
//! A [`RateCard`] turns a measured [`Usage`] into micro-`$MATA`. It lives apart
//! from measurement so a host can price however it likes, and so a developer can
//! get an honest **pre-deploy cost estimate** before spending anything.

use crate::usage::Usage;

const GIB: u128 = 1 << 30;
/// A 30-day month, in seconds.
const MONTH_SECONDS: u128 = 2_592_000;
const MEGA: u128 = 1_000_000;

/// Prices per resource unit, in micro-`$MATA`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateCard {
    /// Per GiB held for one month.
    pub storage_per_gib_month: u64,
    /// Per million fuel units of compute.
    pub compute_per_megafuel: u64,
    /// Per function invocation (a fixed per-call overhead).
    pub compute_per_invocation: u64,
    /// Per GiB of transit served.
    pub transit_per_gib: u64,
}

impl RateCard {
    /// The micro-`$MATA` price of one measured amount. Multiplies before dividing
    /// so sub-unit usage is not silently rounded to zero.
    pub fn price(&self, usage: &Usage) -> u64 {
        match usage {
            Usage::Storage { byte_seconds } => {
                (*byte_seconds * self.storage_per_gib_month as u128 / (GIB * MONTH_SECONDS)) as u64
            }
            Usage::Compute { fuel, invocations } => {
                let by_fuel = (*fuel as u128 * self.compute_per_megafuel as u128 / MEGA) as u64;
                by_fuel + invocations * self.compute_per_invocation
            }
            Usage::Transit { bytes_served } => {
                (*bytes_served as u128 * self.transit_per_gib as u128 / GIB) as u64
            }
        }
    }

    /// The total estimated cost of a set of amounts — a pre-deploy quote.
    pub fn estimate(&self, usages: &[Usage]) -> u64 {
        usages.iter().map(|u| self.price(u)).sum()
    }
}
