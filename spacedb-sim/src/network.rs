//! The network model — the "noise".
//!
//! Every message a replica sends is subject to latency (a base plus random jitter),
//! a drop probability, and — when active — a partition that severs the two sides.
//! All randomness flows through the simulation's single seeded [`Rng`], so the noise
//! is reproducible.

use crate::rng::Rng;

/// Per-message network behaviour.
#[derive(Clone, Copy, Debug)]
pub struct NetworkModel {
    /// Minimum delivery delay, in ticks.
    pub latency_base: u64,
    /// Extra uniform jitter in `[0, latency_jitter]` ticks.
    pub latency_jitter: u64,
    /// Probability a message is dropped in flight.
    pub drop_prob: f64,
}

impl NetworkModel {
    /// A fast, lossless network.
    pub fn perfect() -> Self {
        Self {
            latency_base: 1,
            latency_jitter: 0,
            drop_prob: 0.0,
        }
    }

    /// A network with the given mean latency window and loss.
    pub fn new(latency_base: u64, latency_jitter: u64, drop_prob: f64) -> Self {
        Self {
            latency_base,
            latency_jitter,
            drop_prob,
        }
    }

    /// Draw a delivery delay.
    pub fn delay(&self, rng: &mut Rng) -> u64 {
        self.latency_base + rng.below(self.latency_jitter + 1)
    }

    /// Whether this message is dropped.
    pub fn drops(&self, rng: &mut Rng) -> bool {
        rng.chance(self.drop_prob)
    }
}

/// A two-sided network partition: replicas on different sides cannot reach each
/// other until it heals.
#[derive(Clone, Debug)]
pub struct Partition {
    /// Side (0 or 1) per replica index.
    side: Vec<u8>,
}

impl Partition {
    /// Put the replicas listed in `group_a` on one side and everyone else on the
    /// other.
    pub fn split(replicas: usize, group_a: &[usize]) -> Self {
        let mut side = vec![0u8; replicas];
        for &r in group_a {
            if r < replicas {
                side[r] = 1;
            }
        }
        Self { side }
    }

    /// Whether `a` and `b` can currently reach each other.
    pub fn connected(&self, a: usize, b: usize) -> bool {
        self.side.get(a) == self.side.get(b)
    }
}
