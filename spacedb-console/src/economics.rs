//! Board 3 — Economics. "Are we making money, and is a customer about to overspend?"

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use spacedb_meter::Resource;

use crate::observe::{AgentBudgetObs, SettledObs};

/// Revenue split across the three rails.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RailBreakdown {
    pub storage: u64,
    pub compute: u64,
    pub transit: u64,
}

/// One agent's budget burn-down.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetStatus {
    pub agent: String,
    pub remaining: u64,
    pub limit: u64,
    pub used_pct: u8,
    pub exhausted: bool,
    pub low: bool,
}

/// The business board.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Economics {
    /// Total micro-$MATA credited to hosting homes.
    pub revenue_micro_mata: u64,
    /// micro-$MATA billed per customer mID.
    pub spend_by_customer: BTreeMap<String, u64>,
    pub per_rail: RailBreakdown,
    pub budgets: Vec<BudgetStatus>,
    pub unsettled_claims: usize,
}

/// Roll up settled receipts and agent budgets. `budget_low_pct` is the
/// remaining-percentage threshold below which an agent is flagged low.
pub fn rollup_economics(
    settled: &[SettledObs],
    budgets: &[AgentBudgetObs],
    unsettled_claims: usize,
    budget_low_pct: u8,
) -> Economics {
    let mut revenue = 0u64;
    let mut per_rail = RailBreakdown::default();
    let mut spend_by_customer: BTreeMap<String, u64> = BTreeMap::new();

    for r in settled {
        revenue += r.micro_mata;
        *spend_by_customer.entry(r.settles_to_did.clone()).or_default() += r.micro_mata;
        match r.resource {
            Resource::Storage => per_rail.storage += r.micro_mata,
            Resource::Compute => per_rail.compute += r.micro_mata,
            Resource::Transit => per_rail.transit += r.micro_mata,
        }
    }

    let budgets = budgets
        .iter()
        .map(|b| {
            let used = b.limit.saturating_sub(b.remaining);
            let used_pct = if b.limit > 0 {
                (used * 100 / b.limit) as u8
            } else {
                100
            };
            let low = b.limit > 0 && b.remaining * 100 / b.limit <= budget_low_pct as u64;
            BudgetStatus {
                agent: b.agent.clone(),
                remaining: b.remaining,
                limit: b.limit,
                used_pct,
                exhausted: b.remaining == 0,
                low,
            }
        })
        .collect();

    Economics {
        revenue_micro_mata: revenue,
        spend_by_customer,
        per_rail,
        budgets,
        unsettled_claims,
    }
}
