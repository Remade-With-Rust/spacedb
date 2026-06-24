//! The dashboard — the four boards assembled from one snapshot of observations.
//!
//! A Dioxus/WASM shell binds [`Dashboard`] fields to components; [`render_text`]
//! produces the same view as an operator snapshot, so the model is demonstrable
//! (and tested) with no browser.

use serde::{Deserialize, Serialize};

use crate::access::{rollup_access, AccessOverview};
use crate::alerts::{derive_alerts, Alert, AlertThresholds, Severity};
use crate::economics::{rollup_economics, Economics};
use crate::fleet::{assess_fleet, FleetHealth, HealthStatus};
use crate::observe::{
    AgentBudgetObs, AuditObs, CapabilityObs, HomeObs, LagObs, SettledObs, ShardObs, StrongObs,
};

/// One snapshot of everything the console observes.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Observations {
    pub homes: Vec<HomeObs>,
    pub shards: Vec<ShardObs>,
    pub strong: Vec<StrongObs>,
    pub lags: Vec<LagObs>,
    pub capabilities: Vec<CapabilityObs>,
    pub audit: Vec<AuditObs>,
    pub settled: Vec<SettledObs>,
    pub budgets: Vec<AgentBudgetObs>,
    pub unsettled_claims: usize,
}

/// Operator-tunable thresholds for the whole dashboard.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub now: u64,
    pub alerts: AlertThresholds,
    pub expiry_window: u64,
}

impl Config {
    pub fn at(now: u64) -> Self {
        Self {
            now,
            alerts: AlertThresholds::default(),
            expiry_window: 7 * 86_400, // a week
        }
    }
}

/// The four boards.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Dashboard {
    pub health: FleetHealth,
    pub alerts: Vec<Alert>,
    pub economics: Economics,
    pub access: AccessOverview,
}

impl Dashboard {
    /// Compute the whole dashboard from a snapshot.
    pub fn assemble(obs: &Observations, config: &Config) -> Dashboard {
        let health = assess_fleet(
            &obs.homes,
            &obs.shards,
            &obs.strong,
            &obs.lags,
            config.alerts.lag_ops_warn,
        );
        let alerts = derive_alerts(
            &obs.homes,
            &obs.shards,
            &obs.strong,
            &obs.lags,
            &obs.budgets,
            &config.alerts,
        );
        let economics = rollup_economics(
            &obs.settled,
            &obs.budgets,
            obs.unsettled_claims,
            config.alerts.budget_low_pct,
        );
        let access = rollup_access(&obs.capabilities, &obs.audit, config.now, config.expiry_window);
        Dashboard {
            health,
            alerts,
            economics,
            access,
        }
    }

    /// The number of alerts at or above `Critical` — i.e. the page-me count.
    pub fn critical_count(&self) -> usize {
        self.alerts
            .iter()
            .filter(|a| a.severity == Severity::Critical)
            .count()
    }

    /// A plain-text operator snapshot (what the Dioxus shell renders graphically).
    pub fn render_text(&self) -> String {
        let h = &self.health;
        let badge = match h.status {
            HealthStatus::Green => "GREEN",
            HealthStatus::Amber => "AMBER",
            HealthStatus::Red => "RED",
        };
        let mut out = String::new();
        out.push_str(&format!("SpaceDB Operator Console — fleet {badge}\n"));
        out.push_str(&format!(
            "Fleet:    {}/{} homes online · {} shards ({} under-replicated, {} at-risk, {} lost, {} over-replicated) · {} GiB\n",
            h.homes_online,
            h.homes_total,
            h.shards_total,
            h.shards_under_replicated,
            h.shards_at_risk,
            h.shards_lost,
            h.shards_over_replicated,
            h.bytes_stored / (1 << 30),
        ));
        out.push_str(&format!(
            "Strong:   {}/{} collections with quorum · worst lag {} ops\n",
            h.strong_collections - h.strong_without_quorum,
            h.strong_collections,
            h.worst_lag_ops,
        ));
        out.push_str(&format!(
            "Access:   {} human + {} agent caps · {} expiring soon · {} revoked · {} recent denials\n",
            self.access.active_human,
            self.access.active_agent,
            self.access.expiring_soon,
            self.access.revoked,
            self.access.denied_recent,
        ));
        out.push_str(&format!(
            "Economics: {} micro-$MATA revenue (S {} / C {} / T {}) · {} unsettled claims\n",
            self.economics.revenue_micro_mata,
            self.economics.per_rail.storage,
            self.economics.per_rail.compute,
            self.economics.per_rail.transit,
            self.economics.unsettled_claims,
        ));
        out.push_str(&format!("Alerts:   {} ({} critical)\n", self.alerts.len(), self.critical_count()));
        for a in &self.alerts {
            out.push_str(&format!("  [{:?}] {:?} {} — {}\n", a.severity, a.kind, a.subject, a.detail));
        }
        out
    }
}
