//! Strong tier under partition, with budgeted agents — the linearizability twin.
//!
//! A quorum of members backs uniqueness keys (usernames) and a finite seat pool.
//! Budgeted agents contend for them over time while the network partitions: a window
//! takes a minority-making set of members offline, so the coordinator can't reach a
//! majority and **must refuse** (`Unavailable`).
//!
//! The simulation asserts the safety guarantees hold *regardless* of the chaos:
//! a username is never owned by two agents, seats are never oversold, and the
//! quorum never commits without a majority — it fails safe. It also checks the
//! money backstop: an agent's [`Budget`] is charged per attempt and, once spent,
//! the agent simply stops — it can never overspend. Same scenario + seed ⇒ identical
//! [`StrongReport`].

use std::collections::HashMap;

use spacedb_consistency::{QuorumGroup, StrongResult};
use spacedb_meter::Budget;

use crate::rng::Rng;
use crate::scheduler::Scheduler;

/// Everything that defines a strong-tier stress run.
#[derive(Clone, Debug)]
pub struct StrongScenario {
    pub seed: u64,
    pub members: usize,
    pub clients: usize,
    pub usernames: usize,
    pub seats: u64,
    pub horizon: u64,
    pub op_interval: u64,
    pub partition_interval: u64,
    pub partition_duration: u64,
    /// How many members a partition takes offline (≥ members − majority + 1 forces
    /// loss of quorum).
    pub partition_size: usize,
    pub budget_per_client: u64,
    pub op_cost: u64,
}

impl StrongScenario {
    /// A 5-member quorum, 8 budgeted agents contending for 6 usernames and 20 seats,
    /// with recurring quorum-breaking partitions.
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            members: 5,
            clients: 8,
            usernames: 6,
            seats: 20,
            horizon: 15_000,
            op_interval: 25,
            partition_interval: 1_500,
            partition_duration: 400,
            partition_size: 3, // 5 − 3 = 2 reachable < majority 3 → Unavailable
            budget_per_client: 400_000,
            op_cost: 1_000,
        }
    }
}

enum SEvent {
    Op { client: usize },
    Partition,
    Heal,
}

/// The outcome of a strong-tier stress run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrongReport {
    pub members: usize,
    pub commits: u64,
    pub rejects: u64,
    pub unavailables: u64,
    pub seats_pool: u64,
    pub seats_committed: u64,
    pub final_seats_remaining: u64,
    /// A username was committed to two different owners (must never happen).
    pub uniqueness_violation: bool,
    /// More seats were committed than existed (must never happen).
    pub oversell: bool,
    pub budget_total: u64,
    pub total_charged: u64,
    /// Any agent spent past its budget (must never happen).
    pub overspend: bool,
    pub clients_exhausted: u64,
}

struct Client {
    budget: Budget,
}

/// A running strong-tier stress simulation.
pub struct StrongSim {
    scenario: StrongScenario,
    rng: Rng,
    scheduler: Scheduler<SEvent>,
    quorum: QuorumGroup,
    clients: Vec<Client>,
    owners: HashMap<String, String>,
    commits: u64,
    rejects: u64,
    unavailables: u64,
    seats_committed: u64,
    uniqueness_violation: bool,
    total_charged: u64,
}

impl StrongSim {
    pub fn new(scenario: StrongScenario) -> Self {
        let mut rng = Rng::seed(scenario.seed);
        let member_ids: Vec<String> = (0..scenario.members).map(|i| format!("m{i}")).collect();
        let mut quorum = QuorumGroup::new(member_ids);
        quorum.init_seats("seats", scenario.seats);

        let clients = (0..scenario.clients)
            .map(|_| Client {
                budget: Budget::new(scenario.budget_per_client),
            })
            .collect();

        let mut scheduler = Scheduler::new();
        for i in 0..scenario.clients {
            scheduler.schedule(1 + rng.below(scenario.op_interval), SEvent::Op { client: i });
        }
        scheduler.schedule(scenario.partition_interval, SEvent::Partition);

        Self {
            scenario,
            rng,
            scheduler,
            quorum,
            clients,
            owners: HashMap::new(),
            commits: 0,
            rejects: 0,
            unavailables: 0,
            seats_committed: 0,
            uniqueness_violation: false,
            total_charged: 0,
        }
    }

    pub fn run(&mut self) -> StrongReport {
        while let Some(event) = self.scheduler.pop() {
            if self.scheduler.now() > self.scenario.horizon {
                break;
            }
            self.process(event);
        }

        // Heal everything before the final read so the seat count is observable.
        for i in 0..self.scenario.members {
            self.quorum.heal(&format!("m{i}"));
        }
        let final_seats_remaining = self.quorum.seats_remaining("seats").unwrap_or(0);

        let budget_total = self.scenario.budget_per_client * self.scenario.clients as u64;
        let spent: u64 = self
            .clients
            .iter()
            .map(|c| self.scenario.budget_per_client - c.budget.remaining())
            .sum();
        let clients_exhausted = self
            .clients
            .iter()
            .filter(|c| !c.budget.can_afford(self.scenario.op_cost))
            .count() as u64;

        StrongReport {
            members: self.scenario.members,
            commits: self.commits,
            rejects: self.rejects,
            unavailables: self.unavailables,
            seats_pool: self.scenario.seats,
            seats_committed: self.seats_committed,
            final_seats_remaining,
            uniqueness_violation: self.uniqueness_violation,
            oversell: self.seats_committed > self.scenario.seats,
            budget_total,
            total_charged: self.total_charged,
            // charges must reconcile exactly with what budgets actually lost.
            overspend: self.total_charged != spent || self.total_charged > budget_total,
            clients_exhausted,
        }
    }

    fn process(&mut self, event: SEvent) {
        match event {
            SEvent::Op { client } => {
                let cost = self.scenario.op_cost;
                if !self.clients[client].budget.can_afford(cost) {
                    return; // budget spent — the agent stops (the backstop)
                }
                self.clients[client].budget.charge(cost).expect("affordable");
                self.total_charged += cost;

                let owner = format!("agent{client}");
                if self.rng.chance(0.5) {
                    let key = format!("user{}", self.rng.below(self.scenario.usernames as u64));
                    match self.quorum.claim_unique(&key, owner.as_bytes()) {
                        StrongResult::Committed => {
                            self.commits += 1;
                            if let Some(prev) = self.owners.get(&key) {
                                if prev != &owner {
                                    self.uniqueness_violation = true;
                                }
                            }
                            self.owners.insert(key, owner);
                        }
                        StrongResult::Rejected(_) => self.rejects += 1,
                        StrongResult::Unavailable(_) => self.unavailables += 1,
                    }
                } else {
                    match self.quorum.acquire_seat("seats") {
                        StrongResult::Committed => {
                            self.commits += 1;
                            self.seats_committed += 1;
                        }
                        StrongResult::Rejected(_) => self.rejects += 1,
                        StrongResult::Unavailable(_) => self.unavailables += 1,
                    }
                }

                if self.scheduler.now() < self.scenario.horizon {
                    let delay = self.jitter(self.scenario.op_interval);
                    self.scheduler.schedule(delay, SEvent::Op { client });
                }
            }
            SEvent::Partition => {
                for i in 0..self.scenario.partition_size.min(self.scenario.members) {
                    self.quorum.partition(&format!("m{i}"));
                }
                self.scheduler
                    .schedule(self.scenario.partition_duration, SEvent::Heal);
            }
            SEvent::Heal => {
                for i in 0..self.scenario.members {
                    self.quorum.heal(&format!("m{i}"));
                }
                if self.scheduler.now() < self.scenario.horizon {
                    self.scheduler
                        .schedule(self.scenario.partition_interval, SEvent::Partition);
                }
            }
        }
    }

    fn jitter(&mut self, base: u64) -> u64 {
        1 + base / 2 + self.rng.below(base.max(1))
    }
}
