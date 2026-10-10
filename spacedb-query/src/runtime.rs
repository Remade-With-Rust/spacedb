//! The deterministic, sandboxed WASM runtime that runs a function next to the data.
//!
//! Two rules make a result corroboratable:
//!
//! 1. **No ambient nondeterminism.** The module gets **no WASI** — no clock,
//!    randomness, network, or filesystem. Its only input is the bytes we hand it;
//!    its only output is the bytes it returns. So `f(input)` is pure: same module
//!    + input ⇒ same output, fuel, and peak memory on any host.
//! 2. **Bounded by fuel + memory, never by the wall clock.** Fuel is a
//!    deterministic instruction count, so every honest host either completes
//!    within the same budget or traps at the same point. A wall-clock timeout
//!    would make *completion itself* device-dependent and break corroboration.
//!
//! ## The function ABI (host ⇄ guest)
//!
//! A function module exports:
//! - `memory` — its linear memory,
//! - `alloc(len: i32) -> i32` — a guest allocator returning a writable offset,
//! - `run(in_ptr: i32, in_len: i32) -> i64` — the entry point; the `i64` packs the
//!   output slice as `(out_ptr as u32) << 32 | (out_len as u32)`.
//!
//! No host imports are required — which is exactly what keeps a function
//! deterministic and safe to run on a volunteer device.

use std::collections::HashMap;
use std::sync::Mutex;

use wasmtime::{Config, Engine, Instance, Module, Store, StoreLimits, StoreLimitsBuilder, TypedFunc};

use crate::corroborate::FunctionRun;
use crate::error::{QueryError, QueryResult};

/// Per-invocation bounds. Both are **deterministic** — fuel is an instruction
/// count; the memory cap is in whole MB — so they bound execution identically on
/// every host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunLimits {
    pub max_fuel: u64,
    pub max_mem_mb: u32,
}

impl Default for RunLimits {
    fn default() -> Self {
        Self {
            max_fuel: 100_000_000,
            max_mem_mb: 64,
        }
    }
}

/// One execution: the answer the function produced, and the attestation proving it.
#[derive(Clone, Debug)]
pub struct Execution {
    /// The output bytes — the answer that travels back.
    pub output: Vec<u8>,
    /// The deterministic, content-addressed attestation.
    pub run: FunctionRun,
}

/// Store data — holds the memory limiter the engine consults on every
/// `memory.grow`.
struct HostState {
    limits: StoreLimits,
}

/// How many compiled modules a runtime keeps. A query touches two (map and
/// reduce) and a node serves a handful of deployed functions; past this the
/// cache is simply cleared, so it can never grow without bound.
const MODULE_CACHE_CAP: usize = 64;

/// A reusable compute-to-data engine. Construct once per host; [`run`](Self::run)
/// is stateless apart from a compile cache keyed by the workload hash.
pub struct FunctionRuntime {
    engine: Engine,
    /// Compiled modules by BLAKE3 of their bytes — the workload identity every
    /// run already computes. A map-reduce runs the same two modules once per
    /// shard and once per reduce step; compiling them each time was the whole
    /// cost of the query (cranelift dwarfs the guest's work). `Module` is a
    /// cheap `Arc` handle, and compilation is a pure function of the bytes and
    /// the engine config, so a cached module is indistinguishable from a fresh one.
    modules: Mutex<HashMap<[u8; 32], Module>>,
}

impl FunctionRuntime {
    /// The shared wasmtime engine (fuel-metered, backtrace-free) — reused by the
    /// Database-Function path in [`crate::functions`].
    pub(crate) fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Construct the runtime. Fails as [`QueryError::Engine`] if wasmtime rejects
    /// the configuration on this host — a consumer gets an error to handle, not a
    /// panic.
    pub fn new() -> QueryResult<Self> {
        let mut config = Config::new();
        config.consume_fuel(true);
        // We only need traps as recoverable errors, not stack traces. Capturing a
        // wasm backtrace on trap walks native frames, which aborts on Windows with
        // this slim feature set — and a determinism-bound runtime has no use for it.
        config.wasm_backtrace(false);
        let engine = Engine::new(&config).map_err(|e| QueryError::Engine(e.to_string()))?;
        Ok(Self {
            engine,
            modules: Mutex::new(HashMap::new()),
        })
    }

    /// The compiled module for `module_wasm`, whose hash is `workload_hash`.
    pub(crate) fn module(
        &self,
        workload_hash: &[u8; 32],
        module_wasm: &[u8],
    ) -> QueryResult<Module> {
        let mut modules = self.modules.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(module) = modules.get(workload_hash) {
            return Ok(module.clone());
        }
        let module = Module::new(&self.engine, module_wasm)
            .map_err(|e| QueryError::Compile(e.to_string()))?;
        if modules.len() >= MODULE_CACHE_CAP {
            modules.clear();
        }
        modules.insert(*workload_hash, module.clone());
        Ok(module)
    }

    /// Run `module_wasm`'s `run` entry over `input` under `limits`, returning the
    /// output and its verifiable [`FunctionRun`]. `module_wasm` is the deployed
    /// `.wasm` (hashed as the workload identity).
    pub fn run(
        &self,
        module_wasm: &[u8],
        input: &[u8],
        limits: &RunLimits,
    ) -> QueryResult<Execution> {
        let workload_hash = hash(module_wasm);
        let input_digest = hash(input);

        let module = self.module(&workload_hash, module_wasm)?;

        let max_bytes = limits.max_mem_mb as usize * 1024 * 1024;
        let mut store = Store::new(
            &self.engine,
            HostState {
                limits: StoreLimitsBuilder::new().memory_size(max_bytes).build(),
            },
        );
        store.limiter(|h| &mut h.limits);
        store
            .set_fuel(limits.max_fuel)
            .map_err(|e| QueryError::Fuel(e.to_string()))?;

        let instance = Instance::new(&mut store, &module, &[])
            .map_err(|e| QueryError::Instantiate(e.to_string()))?;

        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or(QueryError::MissingExport("memory"))?;
        let alloc: TypedFunc<i32, i32> = instance
            .get_typed_func(&mut store, "alloc")
            .map_err(|_| QueryError::MissingExport("alloc"))?;
        let run: TypedFunc<(i32, i32), i64> = instance
            .get_typed_func(&mut store, "run")
            .map_err(|_| QueryError::MissingExport("run"))?;

        // alloc + write the input
        let in_len =
            i32::try_from(input.len()).map_err(|_| QueryError::Abi("input too large".into()))?;
        let in_ptr = alloc
            .call(&mut store, in_len)
            .map_err(|e| QueryError::Trap(e.to_string()))?;
        memory
            .write(&mut store, in_ptr as usize, input)
            .map_err(|e| QueryError::Abi(format!("input write: {e}")))?;

        // run, then read the returned (out_ptr, out_len) slice
        let packed = run
            .call(&mut store, (in_ptr, in_len))
            .map_err(|e| QueryError::Trap(e.to_string()))?;
        let packed = packed as u64;
        let out_ptr = (packed >> 32) as usize;
        let out_len = (packed & 0xFFFF_FFFF) as usize;
        let output = crate::functions::read_output(memory.data(&store), out_ptr, out_len)?;
        let output_digest = hash(&output);

        // fuel consumed = budget − remaining; peak ≈ final size (functions don't
        // shrink memory within a single run)
        let remaining = store.get_fuel().map_err(|e| QueryError::Fuel(e.to_string()))?;
        let fuel_used = limits.max_fuel.saturating_sub(remaining);
        let mem_peak_mb = (memory.data_size(&store) / (1024 * 1024)) as u32;

        Ok(Execution {
            output,
            run: FunctionRun {
                workload_hash,
                input_digest,
                output_digest,
                fuel_used,
                mem_peak_mb,
            },
        })
    }
}

fn hash(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}
