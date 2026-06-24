use spacedb_console::*;
fn main() {
    let obs = Observations {
        homes: vec![
            HomeObs { id: "home-ny-1".into(), region: "us-east".into(), online: true },
            HomeObs { id: "home-ny-2".into(), region: "us-east".into(), online: true },
            HomeObs { id: "home-sf-1".into(), region: "us-west".into(), online: false },
        ],
        shards: vec![
            ShardObs { id: "profiles-0".into(), collection: "profiles".into(), reachable_replicas: 3, target_replicas: 3, durable_floor: 1, size_bytes: 4 << 30 },
            ShardObs { id: "photos-7".into(), collection: "photos".into(), reachable_replicas: 2, target_replicas: 3, durable_floor: 1, size_bytes: 60 << 30 },
            ShardObs { id: "ledger-2".into(), collection: "ledger".into(), reachable_replicas: 1, target_replicas: 3, durable_floor: 1, size_bytes: 1 << 30 },
        ],
        strong: vec![
            StrongObs { collection: "usernames".into(), members_online: 3, members_total: 3 },
            StrongObs { collection: "seats".into(), members_online: 1, members_total: 3 },
        ],
        lags: vec![ LagObs { collection: "photos".into(), lag_ops: 340, region: Some("us-west".into()) } ],
        capabilities: vec![
            CapabilityObs { bearer: "did:mata:alice".into(), scope: "profiles".into(), ops: "rw".into(), expiry: Some(1_700_003_600), budget_micro_mata: None, revoked: false },
            CapabilityObs { bearer: "did:agent:assistant".into(), scope: "profiles".into(), ops: "rc".into(), expiry: Some(1_702_592_000), budget_micro_mata: Some(1_000_000), revoked: false },
        ],
        audit: vec![
            AuditObs { actor: "did:agent:assistant".into(), action: "retrieve".into(), at: 1_700_000_000, allowed: true },
            AuditObs { actor: "did:agent:assistant".into(), action: "write".into(), at: 1_700_000_050, allowed: false },
        ],
        settled: vec![
            SettledObs { host_did: "home-ny-1".into(), settles_to_did: "alice".into(), resource: Resource::Storage, micro_mata: 4_200_000 },
            SettledObs { host_did: "home-ny-2".into(), settles_to_did: "alice".into(), resource: Resource::Compute, micro_mata: 380_000 },
            SettledObs { host_did: "home-ny-1".into(), settles_to_did: "bob".into(), resource: Resource::Transit, micro_mata: 95_000 },
        ],
        budgets: vec![
            AgentBudgetObs { agent: "did:agent:assistant".into(), remaining: 40_000, limit: 1_000_000 },
        ],
        unsettled_claims: 12,
    };
    print!("{}", Dashboard::assemble(&obs, &Config::at(1_700_000_100)).render_text());
}
