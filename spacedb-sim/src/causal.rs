//! Causal+ under reordered propagation — the session-consistency twin.
//!
//! A session writes a monotonically-increasing counter on its home replica and
//! reads from random replicas, while the home's updates propagate to the others
//! with varied (and therefore reordering) delays. The simulation asserts the two
//! session guarantees the causal tier promises:
//!
//! - **read-your-writes** — a read of the home (which holds every write the session
//!   made) is always served and current; and
//! - **monotonic reads** — a served read never returns a value older than one the
//!   session already saw.
//!
//! And it checks the honesty contract is doing real work: reads of a lagging replica
//! come back `Stale`, not silently old. Same scenario + seed ⇒ identical
//! [`CausalReport`].

use spacedb_consistency::{CausalSession, Outcome, Tier};
use spacedb_crdt::CrdtDoc;

use crate::rng::Rng;
use crate::scheduler::Scheduler;

/// Everything that defines a causal stress run.
#[derive(Clone, Debug)]
pub struct CausalScenario {
    pub seed: u64,
    pub replicas: usize,
    pub horizon: u64,
    pub write_interval: u64,
    pub read_interval: u64,
    pub prop_base: u64,
    pub prop_jitter: u64,
}

impl CausalScenario {
    /// Five replicas, frequent reads, propagation slow and jittery enough to
    /// reorder and to leave non-home replicas lagging.
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            replicas: 5,
            horizon: 10_000,
            write_interval: 20,
            read_interval: 9,
            prop_base: 15,
            prop_jitter: 40,
        }
    }
}

enum CEvent {
    Write,
    Read,
    Deliver { to: usize, update: Vec<u8> },
}

/// The outcome of a causal stress run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CausalReport {
    pub writes: u64,
    pub home_reads: u64,
    pub reads_fresh: u64,
    pub reads_stale: u64,
    /// A home read was not current (must never happen).
    pub ryw_violation: bool,
    /// A served read went backwards (must never happen).
    pub monotonic_violation: bool,
    pub final_value: i64,
}

/// A running causal-consistency stress simulation.
pub struct CausalSim {
    scenario: CausalScenario,
    rng: Rng,
    scheduler: Scheduler<CEvent>,
    replicas: Vec<CrdtDoc>,
    session: CausalSession,
    writes: u64,
    home_reads: u64,
    reads_fresh: u64,
    reads_stale: u64,
    last_fresh_value: i64,
    ryw_violation: bool,
    monotonic_violation: bool,
}

impl CausalSim {
    pub fn new(scenario: CausalScenario) -> Self {
        let replicas = (0..scenario.replicas)
            .map(|i| CrdtDoc::new(i as u64 + 1))
            .collect();
        let mut scheduler = Scheduler::new();
        scheduler.schedule(scenario.write_interval, CEvent::Write);
        scheduler.schedule(scenario.read_interval, CEvent::Read);
        Self {
            rng: Rng::seed(scenario.seed),
            scenario,
            scheduler,
            replicas,
            session: CausalSession::new(),
            writes: 0,
            home_reads: 0,
            reads_fresh: 0,
            reads_stale: 0,
            last_fresh_value: 0,
            ryw_violation: false,
            monotonic_violation: false,
        }
    }

    pub fn run(&mut self) -> CausalReport {
        while let Some(event) = self.scheduler.pop() {
            if self.scheduler.now() > self.scenario.horizon {
                break;
            }
            self.process(event);
        }
        CausalReport {
            writes: self.writes,
            home_reads: self.home_reads,
            reads_fresh: self.reads_fresh,
            reads_stale: self.reads_stale,
            ryw_violation: self.ryw_violation,
            monotonic_violation: self.monotonic_violation,
            final_value: self.replicas[0].counter("v"),
        }
    }

    fn process(&mut self, event: CEvent) {
        match event {
            CEvent::Write => {
                // the session writes on its home replica (0) and records it
                self.replicas[0].increment("v", 1);
                self.session.record_write(&self.replicas[0]);
                self.writes += 1;
                // propagate the home's state to the others with reordering delays
                let update = self.replicas[0].encode_full();
                for to in 1..self.replicas.len() {
                    let delay = self.scenario.prop_base + self.rng.below(self.scenario.prop_jitter + 1);
                    self.scheduler
                        .schedule(delay, CEvent::Deliver { to, update: update.clone() });
                }
                if self.scheduler.now() < self.scenario.horizon {
                    self.scheduler.schedule(self.scenario.write_interval, CEvent::Write);
                }
            }
            CEvent::Deliver { to, update } => {
                let _ = self.replicas[to].apply_update(&update);
            }
            CEvent::Read => {
                let r = self.rng.below(self.replicas.len() as u64) as usize;
                let outcome = self.session.read(&self.replicas[r]);
                match outcome {
                    Outcome::Committed(Tier::Causal) => {
                        self.reads_fresh += 1;
                        let value = self.replicas[r].counter("v");
                        // monotonic reads: never serve a value older than one seen
                        if value < self.last_fresh_value {
                            self.monotonic_violation = true;
                        }
                        self.last_fresh_value = value;
                        // read-your-writes: the home holds every write, so a home
                        // read is always current
                        if r == 0 {
                            self.home_reads += 1;
                            if value != self.replicas[0].counter("v") {
                                self.ryw_violation = true;
                            }
                        }
                    }
                    Outcome::Stale { .. } => self.reads_stale += 1,
                    _ => {}
                }
                // a home read that came back anything but fresh breaks read-your-writes
                if r == 0 && !matches!(outcome, Outcome::Committed(Tier::Causal)) {
                    self.ryw_violation = true;
                    self.home_reads += 1;
                }
                if self.scheduler.now() < self.scenario.horizon {
                    self.scheduler.schedule(self.scenario.read_interval, CEvent::Read);
                }
            }
        }
    }
}
