//! spacedb-sim S1: convergence under noise, partition, churn — reproducibly.

use spacedb_console::{Config, Dashboard};
use spacedb_sim::{observations, NetworkModel, OfflineSpec, PartitionSpec, Scenario, Simulation};

#[test]
fn a_population_converges_on_a_perfect_network() {
    let report = Simulation::new(Scenario::new(1, 40)).run();
    assert!(report.converged);
    assert_eq!(report.final_value, report.writes as i64);
    assert_eq!(report.worst_lag, 0);
    assert!(report.converged_at.is_some());
    assert!(report.writes > 0);
}

#[test]
fn the_protocol_heals_packet_loss() {
    // a fifth of every message dropped — periodic anti-entropy re-requests the
    // missing deltas, so the population still converges (just over more rounds)
    let mut sc = Scenario::new(2, 40);
    sc.network = NetworkModel::new(15, 10, 0.2);
    let report = Simulation::new(sc).run();
    assert!(report.converged, "anti-entropy must overcome 20% loss");
    assert_eq!(report.final_value, report.writes as i64);
    assert!(report.messages_dropped > 0); // loss really happened
}

#[test]
fn replicas_reconcile_after_a_partition_heals() {
    // split the population in two for the middle of the run; both sides keep
    // writing, then it heals and everything reconciles
    let mut sc = Scenario::new(3, 30);
    sc.network = NetworkModel::new(10, 5, 0.05);
    sc.partition = Some(PartitionSpec {
        group_a: (0..15).collect(),
        at: 200,
        heal_at: 1_500,
    });
    let report = Simulation::new(sc).run();
    assert!(report.converged, "both sides must reconcile after heal");
    assert_eq!(report.final_value, report.writes as i64);
    // it converged only after the heal
    assert!(report.converged_at.unwrap() >= 1_500);
}

#[test]
fn an_offline_replicas_local_writes_survive_its_return() {
    // a replica goes offline (still writing locally), then rejoins and its writes
    // propagate — nothing is lost
    let mut sc = Scenario::new(4, 25);
    sc.network = NetworkModel::new(10, 5, 0.05);
    sc.offline = vec![OfflineSpec { replica: 7, at: 100, until: 1_400 }];
    let report = Simulation::new(sc).run();
    assert!(report.converged);
    assert_eq!(report.final_value, report.writes as i64);
}

#[test]
fn the_same_seed_produces_an_identical_run() {
    let scenario = || {
        let mut sc = Scenario::new(99, 35);
        sc.network = NetworkModel::new(20, 15, 0.15);
        sc.partition = Some(PartitionSpec { group_a: (0..12).collect(), at: 300, heal_at: 1_200 });
        sc
    };
    let a = Simulation::new(scenario()).run();
    let b = Simulation::new(scenario()).run();
    assert_eq!(a, b, "same scenario + seed must be byte-identical");
    // and that determinism includes the noisy bits
    assert!(a.messages_dropped > 0 && a.sync_rounds > 0);
}

#[test]
fn a_different_seed_generally_changes_the_run() {
    let run = |seed| {
        let mut sc = Scenario::new(seed, 35);
        sc.network = NetworkModel::new(20, 15, 0.2);
        Simulation::new(sc).run()
    };
    // both converge, but the noisy path differs
    let a = run(1);
    let b = run(2);
    assert!(a.converged && b.converged);
    assert_ne!(
        (a.messages_dropped, a.sync_rounds, a.converged_at),
        (b.messages_dropped, b.sync_rounds, b.converged_at)
    );
}

#[test]
fn it_scales_to_a_larger_population() {
    let mut sc = Scenario::new(5, 200);
    sc.network = NetworkModel::new(10, 8, 0.1);
    sc.horizon = 40_000;
    let report = Simulation::new(sc).run();
    assert!(report.converged);
    assert_eq!(report.final_value, report.writes as i64);
}

#[test]
fn live_state_renders_into_the_console() {
    let mut sim = Simulation::new(Scenario::new(6, 10));
    let report = sim.run();
    assert!(report.converged);

    // after convergence the console shows every replica online, zero lag
    let dash = Dashboard::assemble(&observations(&sim), &Config::at(0));
    assert_eq!(dash.health.homes_online, 10);
    let snapshot = dash.render_text();
    assert!(snapshot.contains("homes online"));
}
