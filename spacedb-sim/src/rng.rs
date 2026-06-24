//! A seeded, zero-dependency PRNG (SplitMix64).
//!
//! Determinism is the whole point of the simulator: the same seed must produce a
//! byte-identical run. So we own the randomness end to end rather than pulling in
//! `rand` (whose defaults and version churn could change a sequence). SplitMix64 is
//! tiny, fast, and fully specified.

/// A deterministic random source.
#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn seed(seed: u64) -> Self {
        Self { state: seed }
    }

    /// The next 64-bit value (SplitMix64).
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `[0, n)`. Returns 0 if `n == 0`.
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next_u64() % n
        }
    }

    /// A float in `[0, 1)` with 53 bits of precision.
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// True with probability `p` (clamped to `[0, 1]`).
    pub fn chance(&mut self, p: f64) -> bool {
        if p <= 0.0 {
            false
        } else if p >= 1.0 {
            true
        } else {
            self.unit() < p
        }
    }
}
