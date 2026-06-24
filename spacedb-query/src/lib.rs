#![forbid(unsafe_code)]
//! # spacedb-query — SpaceDB Layer 4 (compute-to-data)
//!
//! The environment forbids hauling data to a central brain, so SpaceDB inverts
//! it: **the query travels to the data; only the answer travels back.** A query
//! is a deterministic, fuel/memory-bounded WASM function that runs on the node
//! holding the data — and because the runtime is deterministic, the result is
//! **corroboratable**: a host that returns a different answer than its peers is
//! caught.
//!
//! M6-S1 ships the verifiable-compute core: [`FunctionRuntime`] (deterministic,
//! no-WASI WASM execution), the [`FunctionRun`] attestation, and [`corroborate`].
//! The partition-aware planner + pinned-snapshot reads (S2) and the on-node vector
//! index (`spacedb-vector`, S3) build on this.
//!
//! Open-core (MIT): built on `wasmtime` directly — the engine MATA's
//! `maestro-fn-runtime` wraps — with no MATA dependency. Fanning corroboration
//! across independent marketplace hosts is the MATA seam; the runtime, the
//! attestation, and the comparison are here.

mod error;
pub use error::{QueryError, QueryResult};

mod corroborate;
pub use corroborate::{corroborate, Corroboration, FunctionRun};

mod runtime;
pub use runtime::{Execution, FunctionRuntime, RunLimits};

mod snapshot;
pub use snapshot::Snapshot;

mod planner;
pub use planner::{run_query, Coverage, QueryOutcome, QueryPlan, Shard};
