//! Board 2 — Access & Audit. "Is anyone — or any agent — doing something they shouldn't?"

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::observe::{AuditObs, CapabilityObs};

/// One agent's recent footprint in the audit log.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentActivity {
    pub ops: usize,
    pub denied: usize,
}

/// The security board.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessOverview {
    pub active_human: usize,
    pub active_agent: usize,
    pub revoked: usize,
    pub expiring_soon: usize,
    /// Per-agent activity from the audit log (agents only — the ones acting
    /// autonomously and worth watching).
    pub agent_activity: BTreeMap<String, AgentActivity>,
    pub denied_recent: usize,
}

/// Roll up active capabilities and audit activity. A capability counts as
/// "expiring soon" if it expires within `expiry_window` of `now`.
pub fn rollup_access(
    capabilities: &[CapabilityObs],
    audit: &[AuditObs],
    now: u64,
    expiry_window: u64,
) -> AccessOverview {
    let mut active_human = 0;
    let mut active_agent = 0;
    let mut revoked = 0;
    let mut expiring_soon = 0;

    for c in capabilities {
        if c.revoked {
            revoked += 1;
            continue;
        }
        if c.is_agent() {
            active_agent += 1;
        } else {
            active_human += 1;
        }
        if c.expires_within(now, expiry_window) {
            expiring_soon += 1;
        }
    }

    let mut agent_activity: BTreeMap<String, AgentActivity> = BTreeMap::new();
    let mut denied_recent = 0;
    for entry in audit {
        if !entry.allowed {
            denied_recent += 1;
        }
        if entry.is_agent() {
            let a = agent_activity.entry(entry.actor.clone()).or_default();
            a.ops += 1;
            if !entry.allowed {
                a.denied += 1;
            }
        }
    }

    AccessOverview {
        active_human,
        active_agent,
        revoked,
        expiring_soon,
        agent_activity,
        denied_recent,
    }
}
