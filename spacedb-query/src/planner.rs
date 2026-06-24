//! The partition-aware planner: map-reduce across shards, with **honest coverage**.
//!
//! A query is a `map` function run on **each shard, next to its data** (only the
//! partial result travels back), and a `reduce` function that combines partials.
//! Because `reduce` is associative and commutative, partials from a
//! partition-degraded set still compose — and when some shards are unreachable,
//! the result is returned **flagged with coverage** ("computed over 4/5 shards"),
//! never silently under-reported.
//!
//! Each map runs on the verifiable [`FunctionRuntime`](crate::FunctionRuntime), so
//! its [`FunctionRun`](crate::FunctionRun) attestation comes back for corroboration
//! / audit. Fanning a map across *redundant* hosts and comparing their
//! attestations is the MATA seam; this planner is the single-coordinator core.

use crate::corroborate::FunctionRun;
use crate::error::QueryResult;
use crate::runtime::{FunctionRuntime, RunLimits};
use crate::snapshot::Snapshot;

/// One shard of a query's data: a pinned snapshot, on a host that may be down.
#[derive(Clone, Debug)]
pub struct Shard {
    pub id: String,
    pub snapshot: Snapshot,
    pub reachable: bool,
}

impl Shard {
    /// A reachable shard.
    pub fn new(id: impl Into<String>, snapshot: Snapshot) -> Self {
        Self {
            id: id.into(),
            snapshot,
            reachable: true,
        }
    }

    /// Mark the shard's host as down (its data won't contribute).
    pub fn unreachable(mut self) -> Self {
        self.reachable = false;
        self
    }
}

/// The query: a `map` over each shard's snapshot and an associative/commutative
/// `reduce` over the partials, both deployed as deterministic WASM.
pub struct QueryPlan<'a> {
    pub map_wasm: &'a [u8],
    pub reduce_wasm: &'a [u8],
    pub limits: RunLimits,
}

/// How much of the dataset the result actually covers — the honesty contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Coverage {
    pub shards_total: usize,
    pub shards_computed: usize,
}

impl Coverage {
    /// Whether every shard contributed (no partition gaps).
    pub fn is_complete(&self) -> bool {
        self.shards_total > 0 && self.shards_computed == self.shards_total
    }

    /// How many shards were unreachable.
    pub fn missing(&self) -> usize {
        self.shards_total - self.shards_computed
    }
}

/// The result of a query: the reduced output (if any shard contributed), the
/// coverage, and the per-shard map attestations.
#[derive(Clone, Debug)]
pub struct QueryOutcome {
    pub output: Option<Vec<u8>>,
    pub coverage: Coverage,
    pub map_runs: Vec<FunctionRun>,
}

/// Frame two partials for the `reduce` ABI: `len(a) ‖ a ‖ b`.
fn frame_pair(a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut framed = Vec::with_capacity(4 + a.len() + b.len());
    framed.extend_from_slice(&(a.len() as u32).to_le_bytes());
    framed.extend_from_slice(a);
    framed.extend_from_slice(b);
    framed
}

/// Run `plan` over `shards`: map each reachable shard's snapshot, then reduce the
/// partials. Unreachable shards are skipped (lowering coverage, never failing the
/// query); a map/reduce error on a *reachable* shard propagates (it signals a
/// systemic deploy/ABI bug, since the same WASM runs everywhere).
pub fn run_query(
    runtime: &FunctionRuntime,
    plan: &QueryPlan,
    shards: &[Shard],
) -> QueryResult<QueryOutcome> {
    // Map phase: run the query next to each reachable shard's data.
    let mut partials: Vec<Vec<u8>> = Vec::new();
    let mut map_runs: Vec<FunctionRun> = Vec::new();
    for shard in shards {
        if !shard.reachable {
            continue;
        }
        let exec = runtime.run(plan.map_wasm, shard.snapshot.bytes(), &plan.limits)?;
        partials.push(exec.output);
        map_runs.push(exec.run);
    }

    let coverage = Coverage {
        shards_total: shards.len(),
        shards_computed: partials.len(),
    };

    // Reduce phase: fold the partials. Order-independent (reduce is commutative).
    let output = if partials.is_empty() {
        None
    } else {
        let mut acc = partials[0].clone();
        for partial in &partials[1..] {
            let exec = runtime.run(plan.reduce_wasm, &frame_pair(&acc, partial), &plan.limits)?;
            acc = exec.output;
        }
        Some(acc)
    };

    Ok(QueryOutcome {
        output,
        coverage,
        map_runs,
    })
}
