//! The verifiable result — *trust the math, not the machine*.
//!
//! A [`FunctionRun`] is the deterministic attestation a host emits for one
//! execution: the workload it ran, the input it ran on, the output it produced,
//! and the resources it consumed — all content-addressed. Because the runtime is
//! deterministic (no ambient nondeterminism, bounded by fuel not wall-clock), two
//! honest hosts running the same workload on the same input produce **identical**
//! attestations. [`corroborate`] compares two and returns whether they agree, so
//! a host that returns a different answer than its peers is caught.
//!
//! S1 ships the attestation + the comparison. Fanning a query across N independent
//! hosts and gathering their attestations is the MATA seam (M6-S2+); the property
//! it relies on — that an honest run is reproducible — lives here.

use serde::{Deserialize, Serialize};

/// The deterministic record of one function execution. The fields are exactly
/// what must match across honest hosts; nothing device-dependent (e.g. wall time)
/// is included.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionRun {
    /// BLAKE3 of the WASM module — the workload's identity.
    pub workload_hash: [u8; 32],
    /// BLAKE3 of the input bytes.
    pub input_digest: [u8; 32],
    /// BLAKE3 of the output bytes — what the function actually computed.
    pub output_digest: [u8; 32],
    /// Fuel consumed (a deterministic instruction count).
    pub fuel_used: u64,
    /// Peak linear memory, in whole MB.
    pub mem_peak_mb: u32,
}

/// The verdict of comparing two attestations of the *same* workload + input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corroboration {
    /// The two runs match on every deterministic field — the result is trusted.
    Agree,
    /// The two runs diverge — at least one host is wrong (or malicious).
    Disagree,
}

/// Compare two attestations. Honest runs of the same `(workload, input)` agree on
/// all fields; any divergence (a wrong output, a padded fuel count) disagrees.
pub fn corroborate(a: &FunctionRun, b: &FunctionRun) -> Corroboration {
    if a == b {
        Corroboration::Agree
    } else {
        Corroboration::Disagree
    }
}
