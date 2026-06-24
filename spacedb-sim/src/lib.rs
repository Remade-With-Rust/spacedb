#![forbid(unsafe_code)]
//! # spacedb-sim — a deterministic digital twin of the database
//!
//! A discrete-event simulator that runs a population of **real** [`CrdtDoc`] replicas
//! gossiping the **real** anti-entropy protocol over a modeled network — latency,
//! jitter, packet loss, partitions, and churn — all driven by a single seeded
//! [`Rng`]. The same [`Scenario`] and seed produce a byte-identical [`SimReport`], so
//! the simulator is a reproducible instrument: you can ask "do 500 replicas still
//! converge under 20% loss and a mid-run partition?" and get the same answer every
//! time, then bisect a regression against it.
//!
//! It is a twin of the **database** only — open-core, no maestro/Iron-Bank (the full
//! economic twin is a separate, proprietary system). Live state can be
//! rendered through [`observations`] into the `spacedb-console` dashboard.
//!
//! ```
//! use spacedb_sim::{Scenario, Simulation, NetworkModel};
//! let mut sc = Scenario::new(7, 50);
//! sc.network = NetworkModel::new(20, 10, 0.1); // 20±10 tick latency, 10% loss
//! let report = Simulation::new(sc).run();
//! assert!(report.converged);
//! ```
//!
//! [`CrdtDoc`]: spacedb_crdt::CrdtDoc

mod rng;
pub use rng::Rng;

mod scheduler;
pub use scheduler::Scheduler;

mod network;
pub use network::{NetworkModel, Partition};

mod sim;
pub use sim::{OfflineSpec, PartitionSpec, Scenario, SimReport, Simulation};

mod churn;
pub use churn::{ChurnReport, ChurnScenario, ChurnSim};

mod strong;
pub use strong::{StrongReport, StrongScenario, StrongSim};

mod causal;
pub use causal::{CausalReport, CausalScenario, CausalSim};

mod console;
pub use console::{churn_observations, observations};
