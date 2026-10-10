//! Deterministic instruction-count driver: all four simulator twins on fixed
//! seeds (callgrind Ir). The reports are the work-parity anchors. yrs client
//! ids come from fastrand, so it is seeded before any document exists.

use spacedb_sim::{
    CausalScenario, CausalSim, ChurnScenario, ChurnSim, NetworkModel, Scenario, Simulation,
    StrongScenario, StrongSim,
};

fn main() {
    fastrand::seed(11);

    let mut sc = Scenario::new(3, 40);
    sc.network = NetworkModel::new(15, 10, 0.2);
    let sim = Simulation::new(sc).run();
    let causal = CausalSim::new(CausalScenario::new(3)).run();
    let strong = StrongSim::new(StrongScenario::new(3)).run();
    let churn = ChurnSim::new(ChurnScenario::new(3)).run();

    println!("{sim:?} | {causal:?} | {strong:?} | {churn:?}");
}
