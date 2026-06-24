//! Durability under churn — the cold-path twin.
//!
//! A sealed snapshot is erasure-coded into `n = k + parity` shards spread across a
//! fleet of homes. Then the homes **churn**: each cycles offline for a while and
//! rejoins, forever. Two schedulers run against the real durability layer:
//!
//! - **repair** regenerates any shard whose home is unreachable onto a fresh home,
//!   restoring the replica count;
//! - **reclaim** drops the surplus copy a rejoined home leaves behind.
//!
//! The simulation asserts the two things that matter: the snapshot is **never
//! unreconstructable** (reachable shards stay ≥ k at every moment), and
//! over-replication stays **bounded** (reclaim keeps surplus from accumulating).
//! Same scenario + seed ⇒ identical [`ChurnReport`].

use spacedb_durability::{
    allocate, distribute, encode_snapshot, health, reclaim, recover, repair, surplus_shard_count,
    Fleet, Manifest, MemShardStore, Node, Placement, TargetId,
};

use crate::rng::Rng;
use crate::scheduler::Scheduler;

/// Everything that defines a churn run.
#[derive(Clone, Debug)]
pub struct ChurnScenario {
    pub seed: u64,
    pub homes: usize,
    pub domains: usize,
    pub data_shards: usize,
    pub parity_shards: usize,
    pub snapshot_bytes: usize,
    pub horizon: u64,
    /// Mean ticks a home stays online before failing.
    pub fail_interval: u64,
    /// Mean ticks a home stays offline before rejoining.
    pub recovery_time: u64,
    pub repair_interval: u64,
    pub reclaim_interval: u64,
}

impl ChurnScenario {
    /// A baseline: 16 homes across 4 domains, an 8-shard (5+3) snapshot, mild churn.
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            homes: 16,
            domains: 4,
            data_shards: 5,
            parity_shards: 3,
            snapshot_bytes: 4_000,
            horizon: 20_000,
            fail_interval: 2_000,
            recovery_time: 120,
            repair_interval: 15,
            reclaim_interval: 40,
        }
    }

    fn total_shards(&self) -> usize {
        self.data_shards + self.parity_shards
    }
}

enum DEvent {
    Fail { home: usize },
    Recover { home: usize },
    Repair,
    Reclaim,
}

/// The outcome of a churn run — identical for any two runs of the same scenario.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChurnReport {
    pub total_shards: usize,
    pub shards_needed: usize,
    pub repairs_run: u64,
    pub shards_repaired: u64,
    pub reclaims_run: u64,
    pub copies_reclaimed: u64,
    /// The fewest shards reachable at any sampled moment — the durability margin.
    pub min_reachable: usize,
    /// The most surplus copies seen at once.
    pub max_surplus: usize,
    /// True if reachability ever fell below `k` (data temporarily unreconstructable).
    pub ever_below_k: bool,
    /// Whether the snapshot reconstructs byte-for-byte at the end.
    pub recovered_intact: bool,
    /// Surplus remaining after the final cleanup (should be 0).
    pub final_surplus: usize,
}

/// A running durability-under-churn simulation.
pub struct ChurnSim {
    scenario: ChurnScenario,
    rng: Rng,
    scheduler: Scheduler<DEvent>,
    fleet: Fleet,
    manifest: Manifest,
    placement: Placement,
    snapshot: Vec<u8>,
    home_ids: Vec<TargetId>,
    repairs_run: u64,
    shards_repaired: u64,
    reclaims_run: u64,
    copies_reclaimed: u64,
    min_reachable: usize,
    max_surplus: usize,
    ever_below_k: bool,
}

impl ChurnSim {
    /// Build the fleet, seal + erasure-code + place the snapshot, and seed churn.
    pub fn new(scenario: ChurnScenario) -> Self {
        let mut rng = Rng::seed(scenario.seed);
        let snapshot: Vec<u8> = (0..scenario.snapshot_bytes).map(|i| (i * 31 + 7) as u8).collect();
        let (manifest, shards) =
            encode_snapshot(&snapshot, scenario.data_shards, scenario.parity_shards)
                .expect("encode");

        let mut fleet = Fleet::new();
        let mut home_ids = Vec::with_capacity(scenario.homes);
        for i in 0..scenario.homes {
            let id = TargetId(format!("h{i}"));
            let domain = format!("d{}", i % scenario.domains);
            fleet.add(Node::new(id.clone(), domain, MemShardStore::new()));
            home_ids.push(id);
        }

        let placement = allocate(scenario.total_shards(), &fleet.online_targets()).expect("allocate");
        distribute(&manifest, &shards, &placement, &fleet).expect("distribute");

        let mut scheduler = Scheduler::new();
        for i in 0..scenario.homes {
            scheduler.schedule(1 + rng.below(scenario.fail_interval), DEvent::Fail { home: i });
        }
        scheduler.schedule(scenario.repair_interval, DEvent::Repair);
        scheduler.schedule(scenario.reclaim_interval, DEvent::Reclaim);

        let min_reachable = scenario.total_shards();
        Self {
            scenario,
            rng,
            scheduler,
            fleet,
            manifest,
            placement,
            snapshot,
            home_ids,
            repairs_run: 0,
            shards_repaired: 0,
            reclaims_run: 0,
            copies_reclaimed: 0,
            min_reachable,
            max_surplus: 0,
            ever_below_k: false,
        }
    }

    pub fn run(&mut self) -> ChurnReport {
        while let Some(event) = self.scheduler.pop() {
            if self.scheduler.now() > self.scenario.horizon {
                break;
            }
            self.process(event);
        }

        // End sequence: bring everyone home, repair to full health, reclaim the
        // orphans that returning leaves, and verify the snapshot is intact.
        for id in &self.home_ids {
            self.fleet.revive(id);
        }
        if let Ok(report) = repair(&self.manifest, &self.placement, &self.fleet) {
            if !report.repaired_shards.is_empty() {
                self.placement = report.new_placement;
            }
        }
        let _ = reclaim(&self.manifest, &self.placement, &self.fleet);
        let final_surplus = surplus_shard_count(&self.manifest, &self.placement, &self.fleet).unwrap_or(0);
        let recovered_intact = recover(&self.manifest, &self.placement, &self.fleet)
            .map(|bytes| bytes == self.snapshot)
            .unwrap_or(false);

        ChurnReport {
            total_shards: self.manifest.total_shards(),
            shards_needed: self.manifest.shards_needed(),
            repairs_run: self.repairs_run,
            shards_repaired: self.shards_repaired,
            reclaims_run: self.reclaims_run,
            copies_reclaimed: self.copies_reclaimed,
            min_reachable: self.min_reachable,
            max_surplus: self.max_surplus,
            ever_below_k: self.ever_below_k,
            recovered_intact,
            final_surplus,
        }
    }

    fn process(&mut self, event: DEvent) {
        match event {
            DEvent::Fail { home } => {
                self.fleet.kill(&self.home_ids[home]);
                self.sample_margin();
                let delay = self.jitter(self.scenario.recovery_time);
                self.scheduler.schedule(delay, DEvent::Recover { home });
            }
            DEvent::Recover { home } => {
                self.fleet.revive(&self.home_ids[home]);
                let delay = self.jitter(self.scenario.fail_interval);
                self.scheduler.schedule(delay, DEvent::Fail { home });
            }
            DEvent::Repair => {
                self.sample_margin();
                if let Ok(report) = repair(&self.manifest, &self.placement, &self.fleet) {
                    if !report.repaired_shards.is_empty() {
                        self.repairs_run += 1;
                        self.shards_repaired += report.repaired_shards.len() as u64;
                        self.placement = report.new_placement;
                    }
                }
                if self.scheduler.now() < self.scenario.horizon {
                    self.scheduler.schedule(self.scenario.repair_interval, DEvent::Repair);
                }
            }
            DEvent::Reclaim => {
                if let Ok(surplus) = surplus_shard_count(&self.manifest, &self.placement, &self.fleet) {
                    self.max_surplus = self.max_surplus.max(surplus);
                }
                if let Ok(report) = reclaim(&self.manifest, &self.placement, &self.fleet) {
                    if !report.is_empty() {
                        self.reclaims_run += 1;
                        self.copies_reclaimed += report.reclaimed.len() as u64;
                    }
                }
                if self.scheduler.now() < self.scenario.horizon {
                    self.scheduler.schedule(self.scenario.reclaim_interval, DEvent::Reclaim);
                }
            }
        }
    }

    fn sample_margin(&mut self) {
        if let Ok(h) = health(&self.manifest, &self.placement, &self.fleet) {
            self.min_reachable = self.min_reachable.min(h.reachable);
            if h.reachable < h.shards_needed {
                self.ever_below_k = true;
            }
        }
    }

    fn jitter(&mut self, base: u64) -> u64 {
        // base ± ~half, never zero
        let half = base / 2;
        1 + half + self.rng.below(base.max(1))
    }

    /// Per-home liveness, for a console snapshot.
    pub fn home_online(&self) -> Vec<bool> {
        self.home_ids
            .iter()
            .map(|id| self.fleet.node(id).map(|n| n.is_online()).unwrap_or(false))
            .collect()
    }

    /// The snapshot's current reachable shard count (for a console snapshot).
    pub fn reachable_now(&self) -> usize {
        health(&self.manifest, &self.placement, &self.fleet)
            .map(|h| h.reachable)
            .unwrap_or(0)
    }

    /// `(total_shards n, shards_needed k)` for the snapshot.
    pub fn shard_counts(&self) -> (usize, usize) {
        (self.manifest.total_shards(), self.manifest.shards_needed())
    }
}
