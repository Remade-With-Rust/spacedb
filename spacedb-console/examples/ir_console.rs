//! Deterministic instruction-count driver for the operator console and local
//! settlement (callgrind Ir): many observations, repeated actors and customers,
//! assembled and rendered repeatedly. The rendered length, alert counts and
//! settled totals are the work-parity anchors.

use spacedb_console::*;
use spacedb_meter::{LocalSettlement, Settlement};
use spacedb_meter::{RateCard, Usage, UsageClaim};

fn main() {
    let homes = (0..50)
        .map(|i| HomeObs {
            id: format!("home-{i:03}"),
            region: ["us-east", "us-west", "eu"][i % 3].into(),
            online: i % 7 != 0,
        })
        .collect();
    let shards = (0..300u32)
        .map(|i| ShardObs {
            id: format!("shard-{i:04}"),
            collection: ["profiles", "photos", "ledger", "notes"][i as usize % 4].into(),
            reachable_replicas: i % 4,
            target_replicas: 3,
            durable_floor: 1,
            size_bytes: (i as u64 + 1) << 24,
        })
        .collect();
    let strong = (0..10u32)
        .map(|i| StrongObs {
            collection: format!("strong-{i}"),
            members_online: i % 4,
            members_total: 3,
        })
        .collect();
    let lags = (0..20u64)
        .map(|i| LagObs {
            collection: format!("col-{i}"),
            lag_ops: i * 37,
            region: Some("us-west".into()),
        })
        .collect();
    let capabilities = (0..50u64)
        .map(|i| CapabilityObs {
            bearer: format!("did:agent:a{}", i % 12),
            scope: "profiles".into(),
            ops: "rw".into(),
            expiry: Some(1_700_000_000 + i * 600),
            budget_micro_mata: Some(1_000_000),
            revoked: i % 9 == 0,
        })
        .collect();
    let audit = (0..3000u64)
        .map(|i| AuditObs {
            actor: format!("did:agent:a{}", i % 12),
            action: "write".into(),
            at: 1_700_000_000 + i,
            allowed: i % 5 != 0,
        })
        .collect();
    let settled = (0..2000u64)
        .map(|i| SettledObs {
            host_did: format!("home-{:03}", i % 50),
            settles_to_did: format!("customer-{}", i % 25),
            resource: [Resource::Storage, Resource::Compute, Resource::Transit][i as usize % 3],
            micro_mata: 1_000 + i,
        })
        .collect();
    let budgets = (0..30u64)
        .map(|i| AgentBudgetObs {
            agent: format!("did:agent:a{i}"),
            remaining: i * 10_000,
            limit: 1_000_000,
        })
        .collect();
    let obs = Observations {
        homes,
        shards,
        strong,
        lags,
        capabilities,
        audit,
        settled,
        budgets,
        unsettled_claims: 12,
    };

    let mut rendered = 0usize;
    let mut critical = 0usize;
    for _ in 0..20 {
        let dash = Dashboard::assemble(&obs, &Config::at(1_700_000_100));
        critical += dash.critical_count();
        rendered += dash.render_text().len();
    }

    let mut settlement = LocalSettlement::new(RateCard {
        storage_per_gib_month: 5_000_000,
        compute_per_megafuel: 1_000_000,
        compute_per_invocation: 1_000,
        transit_per_gib: 1_000_000,
    });
    let mut total = 0u64;
    for i in 0..2000u64 {
        let claim = UsageClaim::new(
            format!("home-{:03}", i % 50),
            format!("customer-{}", i % 25),
            Usage::compute(1_000_000 + i, 1),
            i,
            i + 60,
        );
        total += settlement.settle(&claim).unwrap().micro_mata;
    }
    println!(
        "rendered {rendered} critical {critical} settled {total} receipts {}",
        settlement.receipts().len()
    );
}
