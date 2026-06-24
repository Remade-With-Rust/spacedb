//! The page-me surface — alerts ranked by severity.
//!
//! Everything an operator should be interrupted for, derived from the same
//! observations, sorted worst-first.

use serde::{Deserialize, Serialize};

use crate::observe::{AgentBudgetObs, HomeObs, LagObs, ShardObs, StrongObs};

/// Sort order is the variant order: `Critical` < `Warning` < `Info`, so sorting
/// ascending puts the worst first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    Critical,
    Warning,
    Info,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlertKind {
    ShardLost,
    QuorumLost,
    ShardAtRisk,
    ShardUnderReplicated,
    ShardOverReplicated,
    HomeOffline,
    ReplicaLagHigh,
    AgentBudgetExhausted,
    AgentBudgetLow,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Alert {
    pub severity: Severity,
    pub kind: AlertKind,
    pub subject: String,
    pub detail: String,
}

/// Thresholds for what counts as alert-worthy.
#[derive(Clone, Copy, Debug)]
pub struct AlertThresholds {
    /// Lag (ops) at or above which a replica is "behind".
    pub lag_ops_warn: u64,
    /// Remaining-budget percentage at or below which an agent is "low".
    pub budget_low_pct: u8,
}

impl Default for AlertThresholds {
    fn default() -> Self {
        Self {
            lag_ops_warn: 100,
            budget_low_pct: 10,
        }
    }
}

/// Derive the full alert list, sorted worst-first.
pub fn derive_alerts(
    homes: &[HomeObs],
    shards: &[ShardObs],
    strong: &[StrongObs],
    lags: &[LagObs],
    budgets: &[AgentBudgetObs],
    thresholds: &AlertThresholds,
) -> Vec<Alert> {
    let mut alerts = Vec::new();

    for s in shards {
        if s.lost() {
            alerts.push(Alert {
                severity: Severity::Critical,
                kind: AlertKind::ShardLost,
                subject: s.id.clone(),
                detail: format!(
                    "{}: {} reachable < {} required to reconstruct",
                    s.collection, s.reachable_replicas, s.durable_floor
                ),
            });
        } else if s.at_risk() {
            alerts.push(Alert {
                severity: Severity::Critical,
                kind: AlertKind::ShardAtRisk,
                subject: s.id.clone(),
                detail: format!(
                    "{}: {} reachable, at the durability floor — one more loss is data loss",
                    s.collection, s.reachable_replicas
                ),
            });
        } else if s.under_replicated() {
            alerts.push(Alert {
                severity: Severity::Warning,
                kind: AlertKind::ShardUnderReplicated,
                subject: s.id.clone(),
                detail: format!(
                    "{}: {}/{} replicas",
                    s.collection, s.reachable_replicas, s.target_replicas
                ),
            });
        }
        // Cost, not danger: surplus copies an operator can reclaim.
        if s.over_replicated() {
            alerts.push(Alert {
                severity: Severity::Warning,
                kind: AlertKind::ShardOverReplicated,
                subject: s.id.clone(),
                detail: format!(
                    "{}: {} surplus replicas ({}/{}) — reclaimable",
                    s.collection,
                    s.excess(),
                    s.reachable_replicas,
                    s.target_replicas
                ),
            });
        }
    }

    for s in strong {
        if !s.has_quorum() {
            alerts.push(Alert {
                severity: Severity::Critical,
                kind: AlertKind::QuorumLost,
                subject: s.collection.clone(),
                detail: format!(
                    "{}/{} quorum members online — strong writes are Unavailable",
                    s.members_online, s.members_total
                ),
            });
        }
    }

    for h in homes.iter().filter(|h| !h.online) {
        alerts.push(Alert {
            severity: Severity::Warning,
            kind: AlertKind::HomeOffline,
            subject: h.id.clone(),
            detail: format!("home in {} is offline", h.region),
        });
    }

    for l in lags.iter().filter(|l| l.lag_ops >= thresholds.lag_ops_warn) {
        alerts.push(Alert {
            severity: Severity::Warning,
            kind: AlertKind::ReplicaLagHigh,
            subject: l.collection.clone(),
            detail: format!("{} ops behind", l.lag_ops),
        });
    }

    for b in budgets {
        if b.remaining == 0 {
            alerts.push(Alert {
                severity: Severity::Warning,
                kind: AlertKind::AgentBudgetExhausted,
                subject: b.agent.clone(),
                detail: "budget exhausted — further priced ops will be refused".into(),
            });
        } else if b.limit > 0 && b.remaining * 100 / b.limit <= thresholds.budget_low_pct as u64 {
            alerts.push(Alert {
                severity: Severity::Info,
                kind: AlertKind::AgentBudgetLow,
                subject: b.agent.clone(),
                detail: format!("{}% of budget remaining", b.remaining * 100 / b.limit),
            });
        }
    }

    alerts.sort_by(|a, b| a.severity.cmp(&b.severity).then(a.subject.cmp(&b.subject)));
    alerts
}
