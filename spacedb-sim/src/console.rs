//! Bridge the simulation's live state into `spacedb-console` observations, so the
//! operator dashboard can render a board straight from a running twin.

use spacedb_console::{HomeObs, LagObs, Observations, ShardObs};

use crate::churn::ChurnSim;
use crate::sim::Simulation;

/// Build a console snapshot of the current simulation state: each replica as a
/// home, and the worst convergence lag as the shared collection's lag.
pub fn observations(sim: &Simulation) -> Observations {
    let homes = sim
        .replica_online()
        .iter()
        .enumerate()
        .map(|(i, &online)| HomeObs {
            id: format!("replica-{i}"),
            region: "sim".into(),
            online,
        })
        .collect();
    let lags = vec![LagObs {
        collection: "shared".into(),
        lag_ops: sim.current_worst_lag(),
        region: None,
    }];
    Observations {
        homes,
        lags,
        ..Default::default()
    }
}

/// Build a console snapshot of a churn simulation: each home, and the sealed
/// snapshot as one shard entry whose reachable/target/floor map to the durability
/// layer's `(reachable, n, k)` — so the console's own under-replicated / at-risk /
/// lost logic reports the snapshot's health.
pub fn churn_observations(sim: &ChurnSim) -> Observations {
    let homes = sim
        .home_online()
        .iter()
        .enumerate()
        .map(|(i, &online)| HomeObs {
            id: format!("home-{i}"),
            region: "sim".into(),
            online,
        })
        .collect();
    let (total, needed) = sim.shard_counts();
    let shards = vec![ShardObs {
        id: "snapshot".into(),
        collection: "sealed".into(),
        reachable_replicas: sim.reachable_now() as u32,
        target_replicas: total as u32,
        durable_floor: needed as u32,
        size_bytes: 0,
    }];
    Observations {
        homes,
        shards,
        ..Default::default()
    }
}
