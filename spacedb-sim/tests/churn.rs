//! spacedb-sim S2: durability under continuous churn — repair keeps the data
//! reconstructable, reclaim keeps over-replication bounded, reproducibly.

use spacedb_console::{Config, Dashboard, HealthStatus};
use spacedb_sim::{churn_observations, ChurnScenario, ChurnSim};

#[test]
fn the_snapshot_survives_continuous_churn() {
    let report = ChurnSim::new(ChurnScenario::new(1)).run();

    // never unreconstructable: reachable shards stayed at or above k throughout
    assert!(!report.ever_below_k, "data must never fall below k reachable shards");
    assert!(report.min_reachable >= report.shards_needed);
    // and it reconstructs byte-for-byte at the end
    assert!(report.recovered_intact);

    // churn really happened and the repair loop responded
    assert!(report.repairs_run > 0, "churn should have driven repairs");
    assert!(report.shards_repaired > 0);
}

#[test]
fn over_replication_is_reclaimed_and_does_not_accumulate() {
    let report = ChurnSim::new(ChurnScenario::new(1)).run();

    // rejoining homes created surplus copies...
    assert!(report.max_surplus > 0, "transient failures should create surplus");
    assert!(report.copies_reclaimed > 0, "reclaim should have dropped surplus");
    // ...but it never piled up: bounded during the run, and zero after cleanup
    assert!(report.max_surplus <= report.total_shards);
    assert_eq!(report.final_surplus, 0);
}

#[test]
fn the_run_is_deterministic() {
    let a = ChurnSim::new(ChurnScenario::new(42)).run();
    let b = ChurnSim::new(ChurnScenario::new(42)).run();
    assert_eq!(a, b, "same scenario + seed must be identical");
    // a different seed exercises a different churn path
    let c = ChurnSim::new(ChurnScenario::new(43)).run();
    assert_ne!(
        (a.repairs_run, a.copies_reclaimed, a.min_reachable),
        (c.repairs_run, c.copies_reclaimed, c.min_reachable)
    );
}

#[test]
fn repair_keeps_up_with_heavier_churn() {
    // homes fail more often and stay down longer; with extra parity and frequent
    // repair, the data is still never lost
    let mut sc = ChurnScenario::new(7);
    sc.homes = 24;
    sc.data_shards = 6;
    sc.parity_shards = 4; // n = 10, more margin
    sc.fail_interval = 900;
    sc.recovery_time = 150;
    sc.repair_interval = 8;
    let report = ChurnSim::new(sc).run();

    assert!(!report.ever_below_k);
    assert!(report.min_reachable >= report.shards_needed);
    assert!(report.recovered_intact);
    assert_eq!(report.final_surplus, 0);
}

#[test]
fn the_snapshot_health_renders_into_the_console() {
    let mut sim = ChurnSim::new(ChurnScenario::new(2));
    let report = sim.run();
    assert!(report.recovered_intact);

    // after the end-sequence (all home, repaired, reclaimed) the snapshot is fully
    // redundant and the fleet board is green
    let dash = Dashboard::assemble(&churn_observations(&sim), &Config::at(0));
    assert_eq!(dash.health.shards_lost, 0);
    assert_eq!(dash.health.status, HealthStatus::Green);
    assert!(dash.render_text().contains("homes online"));
}
