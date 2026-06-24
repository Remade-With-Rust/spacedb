//! M8-S3: the operator console read-model — the four boards from one snapshot.

use spacedb_console::{
    assess_fleet, derive_alerts, rollup_access, rollup_economics, AlertKind, AlertThresholds,
    AuditObs, CapabilityObs, Config, Dashboard, HealthStatus, HomeObs, LagObs, Observations,
    Resource, SettledObs, Severity, ShardObs, StrongObs, AgentBudgetObs,
};

const NOW: u64 = 1_700_000_000;

fn home(id: &str, online: bool) -> HomeObs {
    HomeObs { id: id.into(), region: "us-east".into(), online }
}

fn shard(id: &str, reachable: u32, target: u32, floor: u32) -> ShardObs {
    ShardObs {
        id: id.into(),
        collection: "profiles".into(),
        reachable_replicas: reachable,
        target_replicas: target,
        durable_floor: floor,
        size_bytes: 1 << 30,
    }
}

// ── fleet health ─────────────────────────────────────────────────────────────

#[test]
fn a_healthy_fleet_is_green() {
    let homes = vec![home("h1", true), home("h2", true)];
    let shards = vec![shard("s1", 3, 3, 1)];
    let h = assess_fleet(&homes, &shards, &[], &[], 100);
    assert_eq!(h.status, HealthStatus::Green);
    assert_eq!(h.bytes_stored, 1 << 30);
}

#[test]
fn under_replication_is_amber_but_loss_and_quorum_failure_are_red() {
    // an under-replicated (but still durable) shard -> Amber
    let amber = assess_fleet(&[home("h1", true)], &[shard("s1", 2, 3, 1)], &[], &[], 100);
    assert_eq!(amber.status, HealthStatus::Amber);
    assert_eq!(amber.shards_under_replicated, 1);

    // a lost shard -> Red
    let lost = assess_fleet(&[home("h1", true)], &[shard("s1", 0, 3, 1)], &[], &[], 100);
    assert_eq!(lost.status, HealthStatus::Red);
    assert_eq!(lost.shards_lost, 1);

    // a strong collection without quorum -> Red even if storage is fine
    let no_quorum = assess_fleet(
        &[home("h1", true)],
        &[shard("s1", 3, 3, 1)],
        &[StrongObs { collection: "usernames".into(), members_online: 1, members_total: 3 }],
        &[],
        100,
    );
    assert_eq!(no_quorum.status, HealthStatus::Red);
    assert_eq!(no_quorum.strong_without_quorum, 1);
}

#[test]
fn over_replication_is_counted_and_alerted_but_does_not_threaten_health() {
    // a rejoined home left a surplus copy: 4 reachable vs a target of 3
    let surplus = shard("photos-3", 4, 3, 1);
    let h = assess_fleet(&[home("h1", true)], &[surplus.clone()], &[], &[], 100);
    assert_eq!(h.shards_over_replicated, 1);
    assert_eq!(h.status, HealthStatus::Green); // data is safe — it's only cost

    let alerts = derive_alerts(&[], &[surplus], &[], &[], &[], &AlertThresholds::default());
    let a = alerts.iter().find(|a| a.kind == AlertKind::ShardOverReplicated).unwrap();
    assert_eq!(a.severity, Severity::Warning);
    assert!(a.detail.contains("reclaimable"));
}

#[test]
fn a_shard_at_the_durability_floor_is_flagged_at_risk() {
    let h = assess_fleet(&[home("h1", true)], &[shard("s1", 1, 3, 1)], &[], &[], 100);
    assert_eq!(h.shards_at_risk, 1);
    assert_eq!(h.status, HealthStatus::Amber);
}

// ── alerts ───────────────────────────────────────────────────────────────────

#[test]
fn alerts_are_sorted_worst_first() {
    let homes = vec![home("h9", false)]; // Warning
    let shards = vec![
        shard("lost-1", 0, 3, 1),  // Critical
        shard("low-1", 2, 3, 1),   // Warning
    ];
    let strong = vec![StrongObs { collection: "usernames".into(), members_online: 1, members_total: 3 }]; // Critical
    let alerts = derive_alerts(&homes, &shards, &strong, &[], &[], &AlertThresholds::default());

    // criticals first
    assert_eq!(alerts.first().unwrap().severity, Severity::Critical);
    let criticals = alerts.iter().filter(|a| a.severity == Severity::Critical).count();
    assert_eq!(criticals, 2); // ShardLost + QuorumLost
    assert!(alerts.iter().any(|a| a.kind == AlertKind::QuorumLost));
    assert!(alerts.iter().any(|a| a.kind == AlertKind::ShardLost));
    // the whole list is non-decreasing in severity
    assert!(alerts.windows(2).all(|w| w[0].severity <= w[1].severity));
}

#[test]
fn an_exhausted_agent_budget_raises_an_alert() {
    let budgets = vec![AgentBudgetObs { agent: "did:agent:rag".into(), remaining: 0, limit: 1_000 }];
    let alerts = derive_alerts(&[], &[], &[], &[], &budgets, &AlertThresholds::default());
    assert!(alerts.iter().any(|a| a.kind == AlertKind::AgentBudgetExhausted));
}

// ── economics ────────────────────────────────────────────────────────────────

#[test]
fn economics_rolls_up_revenue_rails_and_budget_burndown() {
    let settled = vec![
        SettledObs { host_did: "h1".into(), settles_to_did: "alice".into(), resource: Resource::Storage, micro_mata: 5_000 },
        SettledObs { host_did: "h1".into(), settles_to_did: "alice".into(), resource: Resource::Transit, micro_mata: 1_000 },
        SettledObs { host_did: "h2".into(), settles_to_did: "bob".into(), resource: Resource::Compute, micro_mata: 3_000 },
    ];
    let budgets = vec![
        AgentBudgetObs { agent: "did:agent:a".into(), remaining: 50, limit: 1_000 },   // 5% -> low
        AgentBudgetObs { agent: "did:agent:b".into(), remaining: 800, limit: 1_000 },  // healthy
    ];
    let econ = rollup_economics(&settled, &budgets, 2, 10);

    assert_eq!(econ.revenue_micro_mata, 9_000);
    assert_eq!(econ.per_rail.storage, 5_000);
    assert_eq!(econ.per_rail.compute, 3_000);
    assert_eq!(econ.per_rail.transit, 1_000);
    assert_eq!(econ.spend_by_customer.get("alice").copied(), Some(6_000));
    assert_eq!(econ.spend_by_customer.get("bob").copied(), Some(3_000));
    assert_eq!(econ.unsettled_claims, 2);

    let a = econ.budgets.iter().find(|b| b.agent == "did:agent:a").unwrap();
    assert!(a.low && a.used_pct == 95 && !a.exhausted);
    let b = econ.budgets.iter().find(|b| b.agent == "did:agent:b").unwrap();
    assert!(!b.low && b.used_pct == 20);
}

// ── access & audit ───────────────────────────────────────────────────────────

#[test]
fn access_distinguishes_humans_agents_and_tracks_agent_activity() {
    let caps = vec![
        CapabilityObs { bearer: "did:mata:alice".into(), scope: "profiles".into(), ops: "rw".into(), expiry: Some(NOW + 3_600), budget_micro_mata: None, revoked: false }, // human, expiring soon
        CapabilityObs { bearer: "did:agent:rag".into(), scope: "profiles".into(), ops: "rc".into(), expiry: Some(NOW + 30 * 86_400), budget_micro_mata: Some(1_000), revoked: false }, // agent
        CapabilityObs { bearer: "did:agent:old".into(), scope: "x".into(), ops: "r".into(), expiry: None, budget_micro_mata: None, revoked: true }, // revoked
    ];
    let audit = vec![
        AuditObs { actor: "did:agent:rag".into(), action: "read".into(), at: NOW, allowed: true },
        AuditObs { actor: "did:agent:rag".into(), action: "compute".into(), at: NOW, allowed: true },
        AuditObs { actor: "did:agent:rag".into(), action: "write".into(), at: NOW, allowed: false }, // denied
        AuditObs { actor: "did:mata:alice".into(), action: "write".into(), at: NOW, allowed: true },
    ];
    let acc = rollup_access(&caps, &audit, NOW, 7 * 86_400);

    assert_eq!(acc.active_human, 1);
    assert_eq!(acc.active_agent, 1);
    assert_eq!(acc.revoked, 1);
    assert_eq!(acc.expiring_soon, 1); // the human cap
    assert_eq!(acc.denied_recent, 1);

    let rag = acc.agent_activity.get("did:agent:rag").unwrap();
    assert_eq!(rag.ops, 3);
    assert_eq!(rag.denied, 1);
    // human activity is not tracked here (only autonomous agents)
    assert!(!acc.agent_activity.contains_key("did:mata:alice"));
}

// ── the whole dashboard ──────────────────────────────────────────────────────

#[test]
fn the_dashboard_assembles_all_four_boards() {
    let obs = Observations {
        homes: vec![home("h1", true), home("h2", false)],
        shards: vec![shard("s1", 3, 3, 1), shard("s2", 0, 3, 1)], // one lost -> Red
        strong: vec![StrongObs { collection: "usernames".into(), members_online: 2, members_total: 3 }],
        lags: vec![LagObs { collection: "profiles".into(), lag_ops: 250, region: None }],
        capabilities: vec![CapabilityObs {
            bearer: "did:agent:rag".into(), scope: "profiles".into(), ops: "rc".into(),
            expiry: Some(NOW + 3_600), budget_micro_mata: Some(1_000), revoked: false,
        }],
        audit: vec![AuditObs { actor: "did:agent:rag".into(), action: "read".into(), at: NOW, allowed: true }],
        settled: vec![SettledObs { host_did: "h1".into(), settles_to_did: "alice".into(), resource: Resource::Storage, micro_mata: 5_000 }],
        budgets: vec![AgentBudgetObs { agent: "did:agent:rag".into(), remaining: 0, limit: 1_000 }],
        unsettled_claims: 1,
    };
    let dash = Dashboard::assemble(&obs, &Config::at(NOW));

    assert_eq!(dash.health.status, HealthStatus::Red); // a shard is lost
    assert!(dash.critical_count() >= 1);
    assert_eq!(dash.economics.revenue_micro_mata, 5_000);
    assert_eq!(dash.access.active_agent, 1);

    let snapshot = dash.render_text();
    assert!(snapshot.contains("fleet RED"));
    assert!(snapshot.contains("Economics:"));
    assert!(snapshot.contains("ShardLost"));
}
