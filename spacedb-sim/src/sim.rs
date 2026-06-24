//! The simulation — a population of real CRDT replicas gossiping under the network
//! model, driven by the deterministic event loop.
//!
//! Each replica owns a real [`CrdtDoc`]. The workload has every replica increment a
//! shared PN-counter `"writes"`; the global truth is the total number of writes
//! performed, and a replica has *converged* exactly when its counter equals that
//! total — a strong, value-level convergence signal. Replicas reconcile with the
//! **real two-message anti-entropy protocol** ([`SyncMessage`]): a gossip round
//! sends a peer the sender's state vector, and the peer replies with the exact
//! delta the sender lacks ([`CrdtDoc::encode_update_since`]). Because gossip repeats,
//! the protocol heals dropped messages and partitions on its own.

use spacedb_crdt::CrdtDoc;
use spacedb_replica::SyncMessage;

use crate::network::{NetworkModel, Partition};
use crate::rng::Rng;
use crate::scheduler::Scheduler;

/// A window during which one replica is offline (it still writes locally —
/// local-first — and re-syncs on return).
#[derive(Clone, Copy, Debug)]
pub struct OfflineSpec {
    pub replica: usize,
    pub at: u64,
    pub until: u64,
}

/// A network partition window splitting `group_a` from everyone else.
#[derive(Clone, Debug)]
pub struct PartitionSpec {
    pub group_a: Vec<usize>,
    pub at: u64,
    pub heal_at: u64,
}

/// Everything that defines a run. Same scenario + same seed ⇒ identical result.
#[derive(Clone, Debug)]
pub struct Scenario {
    pub seed: u64,
    pub replicas: usize,
    pub horizon: u64,
    /// Writes are generated until this time; after it the system only reconciles.
    pub write_until: u64,
    pub write_interval: u64,
    pub gossip_interval: u64,
    pub probe_interval: u64,
    pub network: NetworkModel,
    pub partition: Option<PartitionSpec>,
    pub offline: Vec<OfflineSpec>,
}

impl Scenario {
    /// A baseline scenario: `replicas` replicas on a perfect network, writing for
    /// the first fifth of a 20 000-tick horizon.
    pub fn new(seed: u64, replicas: usize) -> Self {
        Self {
            seed,
            replicas,
            horizon: 20_000,
            write_until: 1_000,
            write_interval: 10,
            gossip_interval: 5,
            probe_interval: 50,
            network: NetworkModel::perfect(),
            partition: None,
            offline: Vec::new(),
        }
    }

    fn heal_settled_at(&self) -> u64 {
        self.partition.as_ref().map(|p| p.heal_at).unwrap_or(0)
    }

    fn offline_settled_at(&self) -> u64 {
        self.offline.iter().map(|o| o.until).max().unwrap_or(0)
    }
}

enum Event {
    Write { replica: usize },
    Gossip { replica: usize },
    Deliver { to: usize, from: usize, frame: Vec<u8> },
    SetOnline { replica: usize, online: bool },
    Partition,
    Heal,
    Probe,
}

struct Replica {
    doc: CrdtDoc,
    online: bool,
}

/// The outcome of a run — the same for any two runs of the same scenario.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimReport {
    pub replicas: usize,
    pub writes: u64,
    /// Whether every replica's counter equals the global write total.
    pub converged: bool,
    /// The first tick (after writes/partitions settled) at which it converged.
    pub converged_at: Option<u64>,
    pub messages_sent: u64,
    pub messages_dropped: u64,
    pub messages_delivered: u64,
    pub sync_rounds: u64,
    /// The largest number of writes any single replica is still missing.
    pub worst_lag: u64,
    /// The converged value held by replica 0 (equals `writes` on success).
    pub final_value: i64,
}

/// A running simulation.
pub struct Simulation {
    scenario: Scenario,
    rng: Rng,
    scheduler: Scheduler<Event>,
    replicas: Vec<Replica>,
    partition: Option<Partition>,
    sent: u64,
    dropped: u64,
    delivered: u64,
    sync_rounds: u64,
    writes: u64,
    converged_at: Option<u64>,
}

impl Simulation {
    /// Build a simulation and seed its initial events.
    pub fn new(scenario: Scenario) -> Self {
        let mut rng = Rng::seed(scenario.seed);
        let replicas = (0..scenario.replicas)
            .map(|i| Replica {
                doc: CrdtDoc::new(i as u64 + 1),
                online: true,
            })
            .collect();
        let mut scheduler = Scheduler::new();

        // Stagger the first write/gossip per replica so they don't all fire on the
        // same tick (deterministic: rng is consumed in index order).
        for i in 0..scenario.replicas {
            scheduler.schedule(1 + rng.below(scenario.write_interval), Event::Write { replica: i });
            scheduler.schedule(1 + rng.below(scenario.gossip_interval), Event::Gossip { replica: i });
        }
        scheduler.schedule(scenario.probe_interval, Event::Probe);

        if let Some(p) = &scenario.partition {
            scheduler.schedule_at(p.at, Event::Partition);
            scheduler.schedule_at(p.heal_at, Event::Heal);
        }
        for o in &scenario.offline {
            scheduler.schedule_at(o.at, Event::SetOnline { replica: o.replica, online: false });
            scheduler.schedule_at(o.until, Event::SetOnline { replica: o.replica, online: true });
        }

        Self {
            scenario,
            rng,
            scheduler,
            replicas,
            partition: None,
            sent: 0,
            dropped: 0,
            delivered: 0,
            sync_rounds: 0,
            writes: 0,
            converged_at: None,
        }
    }

    /// Run to the horizon and report.
    pub fn run(&mut self) -> SimReport {
        while let Some(event) = self.scheduler.pop() {
            if self.scheduler.now() > self.scenario.horizon {
                break;
            }
            self.process(event);
            // Once converged the system is quiescent (writes have stopped, every
            // replica is current) — stop rather than gossip pointlessly to the
            // horizon. Deterministic: convergence is detected at a fixed tick.
            if self.converged_at.is_some() {
                break;
            }
        }
        SimReport {
            replicas: self.replicas.len(),
            writes: self.writes,
            converged: self.all_converged(),
            converged_at: self.converged_at,
            messages_sent: self.sent,
            messages_dropped: self.dropped,
            messages_delivered: self.delivered,
            sync_rounds: self.sync_rounds,
            worst_lag: self.worst_lag(),
            final_value: self.replicas[0].doc.counter("writes"),
        }
    }

    fn process(&mut self, event: Event) {
        match event {
            Event::Write { replica } => {
                // Local-first: a replica writes whether or not it is online.
                self.replicas[replica].doc.increment("writes", 1);
                self.writes += 1;
                if self.scheduler.now() < self.scenario.write_until {
                    self.scheduler
                        .schedule(self.scenario.write_interval, Event::Write { replica });
                }
            }
            Event::Gossip { replica } => {
                if self.replicas[replica].online {
                    if let Some(peer) = self.pick_peer(replica) {
                        let sv = self.replicas[replica].doc.state_vector();
                        let frame = SyncMessage::StateVector(sv).encode();
                        self.try_send(replica, peer, frame);
                    }
                }
                if self.scheduler.now() < self.scenario.horizon {
                    self.scheduler
                        .schedule(self.scenario.gossip_interval, Event::Gossip { replica });
                }
            }
            Event::Deliver { to, from, frame } => {
                if !self.replicas[to].online {
                    self.dropped += 1; // can't receive while offline
                    return;
                }
                self.delivered += 1;
                match SyncMessage::decode(&frame) {
                    Ok(SyncMessage::StateVector(sv)) => {
                        // Reply with exactly the delta `from` is missing.
                        if let Ok(delta) = self.replicas[to].doc.encode_update_since(&sv) {
                            if !delta.is_empty() {
                                self.sync_rounds += 1;
                                let reply = SyncMessage::Update(delta).encode();
                                self.try_send(to, from, reply);
                            }
                        }
                    }
                    Ok(SyncMessage::Update(update)) => {
                        let _ = self.replicas[to].doc.apply_update(&update);
                    }
                    Err(_) => {}
                }
            }
            Event::SetOnline { replica, online } => {
                self.replicas[replica].online = online;
            }
            Event::Partition => {
                if let Some(p) = &self.scenario.partition {
                    self.partition = Some(Partition::split(self.replicas.len(), &p.group_a));
                }
            }
            Event::Heal => {
                self.partition = None;
            }
            Event::Probe => {
                let now = self.scheduler.now();
                let settled = now >= self.scenario.write_until
                    && now >= self.scenario.heal_settled_at()
                    && now >= self.scenario.offline_settled_at();
                if self.converged_at.is_none() && settled && self.all_online() && self.all_converged() {
                    self.converged_at = Some(now);
                }
                if self.converged_at.is_none() && now < self.scenario.horizon {
                    self.scheduler.schedule(self.scenario.probe_interval, Event::Probe);
                }
            }
        }
    }

    fn try_send(&mut self, from: usize, to: usize, frame: Vec<u8>) {
        self.sent += 1;
        if let Some(p) = &self.partition {
            if !p.connected(from, to) {
                self.dropped += 1; // severed by the partition
                return;
            }
        }
        if self.scenario.network.drops(&mut self.rng) {
            self.dropped += 1;
            return;
        }
        let delay = self.scenario.network.delay(&mut self.rng);
        self.scheduler.schedule(delay, Event::Deliver { to, from, frame });
    }

    fn pick_peer(&mut self, replica: usize) -> Option<usize> {
        let n = self.replicas.len();
        if n < 2 {
            return None;
        }
        let mut peer = self.rng.below(n as u64) as usize;
        if peer == replica {
            peer = (peer + 1) % n;
        }
        Some(peer)
    }

    fn all_online(&self) -> bool {
        self.replicas.iter().all(|r| r.online)
    }

    fn all_converged(&self) -> bool {
        let target = self.writes as i64;
        self.replicas.iter().all(|r| r.doc.counter("writes") == target)
    }

    fn worst_lag(&self) -> u64 {
        let target = self.writes as i64;
        self.replicas
            .iter()
            .map(|r| (target - r.doc.counter("writes")).max(0) as u64)
            .max()
            .unwrap_or(0)
    }

    /// Per-replica liveness (for building a console snapshot).
    pub fn replica_online(&self) -> Vec<bool> {
        self.replicas.iter().map(|r| r.online).collect()
    }

    /// The largest write-deficit across replicas, right now.
    pub fn current_worst_lag(&self) -> u64 {
        self.worst_lag()
    }
}
