//! spacedb-sim S3: consistency & access under stress — strong-tier safety under
//! partition with budgeted agents, and causal guarantees under reordering.

use spacedb_sim::{CausalScenario, CausalSim, StrongScenario, StrongSim};

// ── strong tier under partition ──────────────────────────────────────────────

#[test]
fn the_strong_tier_stays_safe_through_recurring_partitions() {
    let report = StrongSim::new(StrongScenario::new(1)).run();

    // safety — these must hold no matter what the network did
    assert!(!report.uniqueness_violation, "a username was double-committed");
    assert!(!report.oversell, "seats were oversold");
    assert!(report.seats_committed <= report.seats_pool);
    assert_eq!(report.final_seats_remaining, report.seats_pool - report.seats_committed);

    // the fail-safe path was actually exercised, and progress was still made
    assert!(report.unavailables > 0, "partitions should have forced Unavailable");
    assert!(report.commits > 0, "healthy windows should have committed");
}

#[test]
fn agents_cannot_overspend_their_budgets_under_load() {
    let report = StrongSim::new(StrongScenario::new(1)).run();
    assert!(!report.overspend, "charges must reconcile with budgets exactly");
    assert!(report.total_charged <= report.budget_total);
    // the backstop fired: agents ran out and stopped
    assert!(report.clients_exhausted > 0);
}

#[test]
fn a_harsher_partition_schedule_is_still_safe() {
    // partitions that are more frequent and longer
    let mut sc = StrongScenario::new(5);
    sc.partition_interval = 700;
    sc.partition_duration = 500;
    let report = StrongSim::new(sc).run();
    assert!(!report.uniqueness_violation && !report.oversell);
    assert!(report.unavailables > 0);
}

#[test]
fn the_strong_run_is_deterministic() {
    let a = StrongSim::new(StrongScenario::new(9)).run();
    let b = StrongSim::new(StrongScenario::new(9)).run();
    assert_eq!(a, b);
    let c = StrongSim::new(StrongScenario::new(10)).run();
    assert_ne!((a.commits, a.unavailables), (c.commits, c.unavailables));
}

// ── causal+ under reordering ─────────────────────────────────────────────────

#[test]
fn causal_reads_are_consistent_under_reordered_propagation() {
    let report = CausalSim::new(CausalScenario::new(2)).run();

    // the session always read its own writes and never went backwards
    assert!(!report.ryw_violation, "a home read was not current");
    assert!(!report.monotonic_violation, "a served read went backwards");

    // both honesty paths were exercised: fresh reads and honest staleness
    assert!(report.reads_fresh > 0);
    assert!(report.reads_stale > 0, "lagging replicas should report Stale");
    assert!(report.home_reads > 0);
    assert_eq!(report.final_value, report.writes as i64);
}

#[test]
fn the_causal_run_is_deterministic() {
    let a = CausalSim::new(CausalScenario::new(3)).run();
    let b = CausalSim::new(CausalScenario::new(3)).run();
    assert_eq!(a, b);
    let c = CausalSim::new(CausalScenario::new(4)).run();
    assert_ne!((a.reads_stale, a.reads_fresh), (c.reads_stale, c.reads_fresh));
}
