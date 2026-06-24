//! M8-S1: deterministic metering, pricing, budgeting, and the settlement seam.

use spacedb_meter::{
    Budget, LocalSettlement, MeterError, MeterLedger, ProofKind, ProofRef, RateCard, Resource,
    Settled, Settlement, Usage, UsageClaim,
};

const PERIOD_START: u64 = 1_700_000_000;
const PERIOD_END: u64 = 1_700_086_400; // +1 day

/// A round-number rate card that makes the test arithmetic exact.
fn rate_card() -> RateCard {
    RateCard {
        storage_per_gib_month: 5_000_000,   // 5 $MATA / GiB·month
        compute_per_megafuel: 2_000_000,    // 2 $MATA / Mfuel
        compute_per_invocation: 1_000,      // 0.001 $MATA / call
        transit_per_gib: 1_000_000,         // 1 $MATA / GiB
    }
}

// ─── measurement ─────────────────────────────────────────────────────────────

#[test]
fn storage_is_metered_per_replica() {
    // the same bytes for the same time on 3 replicas costs 3x — replication is
    // real cost, metered honestly
    let one = Usage::storage(1_000, 60, 1);
    let three = Usage::storage(1_000, 60, 3);
    assert_eq!(one, Usage::Storage { byte_seconds: 60_000 });
    assert_eq!(three, Usage::Storage { byte_seconds: 180_000 });
}

#[test]
fn transit_bills_the_minimum_both_sides_agree_on() {
    // the server can't inflate (claims 1000, consumer only acks 950 -> 950)
    assert_eq!(Usage::transit(1_000, 950), Usage::Transit { bytes_served: 950 });
    // and the consumer can't either (acks 2000 but server only sent 950 -> 950)
    assert_eq!(Usage::transit(950, 2_000), Usage::Transit { bytes_served: 950 });
}

#[test]
fn compute_carries_fuel_and_invocations() {
    let u = Usage::compute(3_000_000, 2);
    assert_eq!(u, Usage::Compute { fuel: 3_000_000, invocations: 2 });
    assert_eq!(u.resource(), Resource::Compute);
}

// ─── pricing & estimates ─────────────────────────────────────────────────────

#[test]
fn the_rate_card_prices_each_class() {
    let rc = rate_card();
    // 1 GiB held for exactly one month = 5 $MATA
    let gib_month = Usage::Storage {
        byte_seconds: (1u128 << 30) * 2_592_000,
    };
    assert_eq!(rc.price(&gib_month), 5_000_000);
    // 3 Mfuel over 2 calls = 6 + 0.002 $MATA
    assert_eq!(rc.price(&Usage::compute(3_000_000, 2)), 6_000_000 + 2_000);
    // 1 GiB transit = 1 $MATA
    assert_eq!(rc.price(&Usage::Transit { bytes_served: 1 << 30 }), 1_000_000);
}

#[test]
fn a_pre_deploy_estimate_sums_the_plan() {
    let rc = rate_card();
    let plan = [
        Usage::Storage { byte_seconds: (1u128 << 30) * 2_592_000 }, // 5 $MATA
        Usage::Transit { bytes_served: 1 << 30 },                   // 1 $MATA
    ];
    assert_eq!(rc.estimate(&plan), 6_000_000);
}

// ─── budget (the agent spend cap) ────────────────────────────────────────────

#[test]
fn an_agent_cannot_overspend_its_budget() {
    let mut budget = Budget::new(10_000_000);
    assert!(budget.can_afford(6_000_000));
    budget.charge(6_000_000).unwrap();
    assert_eq!(budget.remaining(), 4_000_000);

    // the next op would exceed the cap -> refused, and nothing is deducted
    let err = budget.charge(5_000_000).unwrap_err();
    assert!(matches!(err, MeterError::OverBudget { cost: 5_000_000, remaining: 4_000_000 }));
    assert_eq!(budget.remaining(), 4_000_000);
}

// ─── ledger drain ────────────────────────────────────────────────────────────

#[test]
fn the_ledger_accumulates_then_drains_one_claim_per_class() {
    let mut ledger = MeterLedger::new();
    ledger.record("did:mata:alice", Usage::storage(1_000, 60, 3));
    ledger.record("did:mata:alice", Usage::compute(1_000_000, 1));
    ledger.record("did:mata:alice", Usage::compute(500_000, 1)); // accumulates
    ledger.record("did:mata:alice", Usage::Transit { bytes_served: 2_048 });

    let totals = ledger.totals("did:mata:alice");
    assert_eq!(totals.compute_fuel, 1_500_000);
    assert_eq!(totals.compute_invocations, 2);

    let claims = ledger.drain_claims("did:mata:home-7", PERIOD_START, PERIOD_END);
    assert_eq!(claims.len(), 3); // storage, compute, transit
    assert!(ledger.is_empty()); // drained

    // each claim is amounts-only, attributed to the node, settling to the customer
    for c in &claims {
        assert_eq!(c.node_did, "did:mata:home-7");
        assert_eq!(c.settles_to_did, "did:mata:alice");
        assert_eq!(c.period_end, PERIOD_END);
    }
}

#[test]
fn a_usage_claim_round_trips() {
    let proof = ProofRef { kind: ProofKind::ComputeAttestation, digest: [7u8; 32], at: PERIOD_END };
    let claim = UsageClaim::new(
        "did:mata:home-7",
        "did:mata:alice",
        Usage::compute(2_000_000, 4),
        PERIOD_START,
        PERIOD_END,
    )
    .with_proof(proof);

    let bytes = claim.encode().unwrap();
    assert_eq!(UsageClaim::decode(&bytes).unwrap(), claim);
}

// ─── the settlement seam ─────────────────────────────────────────────────────

#[test]
fn local_settlement_prices_and_tallies_without_a_marketplace() {
    let mut settlement = LocalSettlement::new(rate_card());
    let claim = UsageClaim::new(
        "did:mata:home-7",
        "did:mata:alice",
        Usage::Transit { bytes_served: 1 << 30 },
        PERIOD_START,
        PERIOD_END,
    );
    let receipt = settlement.settle(&claim).unwrap();
    assert_eq!(receipt.micro_mata, 1_000_000);
    assert_eq!(settlement.tallied("did:mata:alice"), 1_000_000);
    assert_eq!(settlement.receipts().len(), 1);
}

/// A stand-in for a host's adapter (e.g. MATA's): it implements the same
/// `Settlement` seam over its own money plane. Here it models the shipped
/// pipeline — counter-sign the claim, then credit the hosting home in $MATA —
/// proving SpaceDB usage flows into maestro/Iron-Bank-style settlement with no
/// dependency in that direction.
#[derive(Default)]
struct HostSettlement {
    rate_card: Option<RateCard>,
    credited_to_host: std::collections::BTreeMap<String, u64>,
    earning_records: Vec<Settled>,
}

impl Settlement for HostSettlement {
    fn settle(&mut self, claim: &UsageClaim) -> Result<Settled, MeterError> {
        // (the host: verify proofs, apply the rate card -> EarningRecord)
        let rc = self.rate_card.ok_or_else(|| MeterError::Settlement("no rate card".into()))?;
        let micro = rc.price(&claim.usage);
        let record = Settled {
            claim_id: claim.claim_id.clone(),
            settles_to_did: claim.settles_to_did.clone(),
            micro_mata: micro,
        };
        // (Iron Bank: credit the hosting node's mID)
        *self.credited_to_host.entry(claim.node_did.clone()).or_default() += micro;
        self.earning_records.push(record.clone());
        Ok(record)
    }
}

#[test]
fn usage_flows_through_the_host_settlement_seam_into_payouts() {
    // 1. SpaceDB measures and accumulates (open-core)
    let mut ledger = MeterLedger::new();
    ledger.record("did:mata:alice", Usage::storage(1u64 << 30, 2_592_000, 1)); // 1 GiB·month
    ledger.record("did:mata:alice", Usage::Transit { bytes_served: 1 << 30 }); // 1 GiB

    // 2. drain to claims and hand each to the host's seam
    let claims = ledger.drain_claims("did:mata:home-7", PERIOD_START, PERIOD_END);
    let mut host = HostSettlement {
        rate_card: Some(rate_card()),
        ..Default::default()
    };
    let mut total = 0;
    for claim in &claims {
        total += host.settle(claim).unwrap().micro_mata;
    }

    // 3. the hosting home is credited the priced sum (5 $MATA storage + 1 $MATA transit)
    assert_eq!(total, 6_000_000);
    assert_eq!(host.credited_to_host.get("did:mata:home-7").copied(), Some(6_000_000));
    assert_eq!(host.earning_records.len(), 2);
}
