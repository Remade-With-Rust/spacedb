//! **Database Functions** — read-write compute-to-data (SpaceDB UX mission M4).
//!
//! A Database Function is developer-authored WASM that runs next to the data with a
//! host context: it **reads a pinned record snapshot** and **writes through a
//! buffered write-set** the caller applies only if the run succeeds. This is the
//! deliberate, designed extension of the runtime's "no host imports" rule — the
//! determinism contract survives because every host call is a pure function of
//! `(snapshot, the run's own prior writes, payload)`:
//!
//! > same module + same input + same snapshot ⇒ same output, same write-set,
//! > same fuel — on any host.
//!
//! Corroboration therefore extends to mutations: redundant executors must agree on
//! [`FunctionCtx::writes_digest`] as well as the output digest, and the caller
//! applies the write-set **once** (keyed by a deterministic run id) no matter how
//! many corroborating nodes ran it.
//!
//! ## The host ABI (one import)
//!
//! In addition to the base ABI (`memory` / `alloc` / `run`), a function module may
//! import exactly one host function:
//!
//! ```wat
//! (import "spacedb" "host_call" (func (param i32 i32 i32 i32) (result i64)))
//! ```
//!
//! `host_call(op_ptr, op_len, payload_ptr, payload_len)` returns the response slice
//! packed `(ptr << 32) | len` (written into guest memory via the guest's `alloc`).
//! Ops and their payloads (`\0` = a NUL byte; collections and record ids must be
//! NUL-free UTF-8):
//!
//! | op       | payload                    | response                                       |
//! |----------|----------------------------|------------------------------------------------|
//! | `get`    | `collection \0 id`         | `[0]` absent · `[1] ++ value` present          |
//! | `query`  | `collection`               | per record, sorted by id: `u32le(id_len) ++ id ++ u32le(val_len) ++ value` |
//! | `put`    | `collection \0 id \0 value`| empty (buffered)                               |
//! | `delete` | `collection \0 id`         | empty (buffered tombstone)                     |
//!
//! A rights violation, a zero-knowledge collection, or a malformed payload **traps
//! the run** — the write-set is discarded with it (all-or-nothing).

use std::collections::{BTreeMap, BTreeSet};

use wasmtime::{
    Caller, Instance, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder, TypedFunc,
};

use crate::corroborate::FunctionRun;
use crate::error::{QueryError, QueryResult};
use crate::runtime::{Execution, FunctionRuntime, RunLimits};

/// A pinned record view: `(collection, record id) → value bytes`, materialized at
/// one frontier. `BTreeMap` so iteration (and every digest over it) is
/// deterministic on every host.
pub type RecordSnapshot = BTreeMap<(String, String), Vec<u8>>;

/// What the invoking capability lets the function do — checked on **every** host
/// call. A function has exactly the caller's rights, never more.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CtxRights {
    pub read: bool,
    pub write: bool,
}

/// The context one function invocation executes in: the pinned snapshot it reads,
/// the write-set it accumulates, the rights it may exercise, and the collections
/// it must be refused (zero-knowledge: the host holds ciphertext it provably
/// cannot compute on — serving it to server-side code would break that promise).
#[derive(Debug)]
pub struct FunctionCtx {
    snapshot: RecordSnapshot,
    snapshot_hash: [u8; 32],
    /// This run's buffered mutations: `Some(value)` = put, `None` = tombstone.
    /// Overlaid on the snapshot for read-your-writes; applied by the caller only
    /// after a successful run.
    overlay: BTreeMap<(String, String), Option<Vec<u8>>>,
    rights: CtxRights,
    denied: BTreeSet<String>,
    /// Scratch `(collection, id)` for `get` lookups: the maps are keyed by owned
    /// strings, so a lookup needs one - reused rather than allocated per call.
    lookup: (String, String),
}

impl FunctionCtx {
    pub fn new(snapshot: RecordSnapshot, rights: CtxRights, denied: BTreeSet<String>) -> Self {
        let snapshot_hash = hash_records(&snapshot);
        Self {
            snapshot,
            snapshot_hash,
            overlay: BTreeMap::new(),
            rights,
            denied,
            lookup: (String::new(), String::new()),
        }
    }

    /// Content hash of the pinned snapshot — bound into the run's `input_digest`
    /// so corroboration can only succeed between runs that saw the same data.
    pub fn snapshot_hash(&self) -> [u8; 32] {
        self.snapshot_hash
    }

    /// The buffered write-set in deterministic order (`Some` = put, `None` = delete).
    pub fn writes(&self) -> &BTreeMap<(String, String), Option<Vec<u8>>> {
        &self.overlay
    }

    /// Digest of the write-set — the second half of mutation corroboration
    /// (executors must agree on it, not just on the output).
    pub fn writes_digest(&self) -> [u8; 32] {
        let mut h = blake3::Hasher::new();
        for ((c, id), v) in &self.overlay {
            frame(&mut h, c.as_bytes());
            frame(&mut h, id.as_bytes());
            match v {
                Some(bytes) => {
                    h.update(&[1]);
                    frame(&mut h, bytes);
                }
                None => {
                    h.update(&[0]);
                }
            }
        }
        *h.finalize().as_bytes()
    }

    /// Total key+value bytes the write-set will add to the store — what the caller
    /// bills as propagated transit when the writes replicate.
    pub fn write_bytes(&self) -> u64 {
        self.overlay
            .iter()
            .map(|((c, id), v)| {
                (c.len() + id.len() + v.as_ref().map(Vec::len).unwrap_or(0)) as u64
            })
            .sum()
    }

    /// Serve one host call into `out` (cleared first). Pure over `(snapshot,
    /// overlay-so-far, op, payload)` — the determinism contract's load-bearing
    /// property. Errors trap the run.
    fn dispatch(&mut self, op: &str, payload: &[u8], out: &mut Vec<u8>) -> Result<(), String> {
        out.clear();
        match op {
            "get" => {
                if !self.rights.read {
                    return Err("rights: get requires database:read".into());
                }
                let (c, id) = split2(payload).ok_or("get: payload must be collection\\0id")?;
                self.check_open(c)?;
                let (lc, lid) = &mut self.lookup;
                lc.clear();
                lc.push_str(c);
                lid.clear();
                lid.push_str(id);
                let value = match self.overlay.get(&self.lookup) {
                    Some(v) => v.as_deref(),                     // read-your-writes
                    None => self.snapshot.get(&self.lookup).map(Vec::as_slice),
                };
                match value {
                    Some(v) => {
                        out.push(1);
                        out.extend_from_slice(v);
                    }
                    None => out.push(0),
                }
                Ok(())
            }
            "query" => {
                if !self.rights.read {
                    return Err("rights: query requires database:read".into());
                }
                let c = std::str::from_utf8(payload).map_err(|_| "query: collection must be UTF-8")?;
                self.check_open(c)?;
                // Snapshot merged with this run's overlay, sorted by id (BTreeMap order).
                // Both maps order by (collection, id), so collection `c` is the
                // contiguous range starting at (c, "") - read just that.
                let (lc, lid) = &mut self.lookup;
                lc.clear();
                lc.push_str(c);
                lid.clear();
                let from = &self.lookup;
                let mut live: BTreeMap<&str, &[u8]> = self
                    .snapshot
                    .range::<(String, String), _>(from..)
                    .take_while(|((col, _), _)| col == c)
                    .map(|((_, id), v)| (id.as_str(), v.as_slice()))
                    .collect();
                for ((col, id), v) in self.overlay.range::<(String, String), _>(from..) {
                    if col != c {
                        break;
                    }
                    match v {
                        Some(bytes) => {
                            live.insert(id.as_str(), bytes.as_slice());
                        }
                        None => {
                            live.remove(id.as_str());
                        }
                    }
                }
                for (id, v) in live {
                    frame_into(out, id.as_bytes());
                    frame_into(out, v);
                }
                Ok(())
            }
            "put" => {
                if !self.rights.write {
                    return Err("rights: put requires database:write".into());
                }
                let (c, id, value) =
                    split3(payload).ok_or("put: payload must be collection\\0id\\0value")?;
                self.check_open(c)?;
                self.overlay.insert((c.to_string(), id.to_string()), Some(value.to_vec()));
                Ok(())
            }
            "delete" => {
                if !self.rights.write {
                    return Err("rights: delete requires database:write".into());
                }
                let (c, id) = split2(payload).ok_or("delete: payload must be collection\\0id")?;
                self.check_open(c)?;
                self.overlay.insert((c.to_string(), id.to_string()), None);
                Ok(())
            }
            other => Err(format!("unknown host op `{other}`")),
        }
    }

    fn check_open(&self, collection: &str) -> Result<(), String> {
        if self.denied.contains(collection) {
            return Err(format!(
                "collection `{collection}` is zero-knowledge: the host cannot compute on it"
            ));
        }
        Ok(())
    }
}

/// One completed function invocation: the execution (output + attestation, with the
/// snapshot hash bound into `input_digest`) and the context carrying the write-set.
#[derive(Debug)]
pub struct FunctionOutcome {
    pub execution: Execution,
    pub ctx: FunctionCtx,
}

struct CtxHostState {
    limits: StoreLimits,
    ctx: FunctionCtx,
    /// The guest's `memory` and `alloc`, resolved once after instantiation so a
    /// host call does not look both up by name (and re-check `alloc`'s
    /// signature) every time. `None` only while the instance is still being
    /// built; `host_call` falls back to the by-name lookup then.
    memory: Option<Memory>,
    alloc: Option<TypedFunc<i32, i32>>,
    /// The response buffer, reused across this run's host calls.
    resp: Vec<u8>,
}

impl FunctionRuntime {
    /// Deploy-time validation: the module may import **only** `spacedb::host_call`
    /// (and must speak the base ABI). Anything else is rejected before it can ever
    /// run — the allowlist is what keeps "deterministic" checkable.
    pub fn validate_function(&self, module_wasm: &[u8]) -> QueryResult<()> {
        let module = Module::new(self.engine(), module_wasm)
            .map_err(|e| QueryError::Compile(e.to_string()))?;
        for import in module.imports() {
            if !(import.module() == "spacedb" && import.name() == "host_call") {
                return Err(QueryError::Abi(format!(
                    "forbidden import `{}::{}` — functions may import only spacedb::host_call",
                    import.module(),
                    import.name()
                )));
            }
        }
        for export in ["memory", "alloc", "run"] {
            if module.get_export(export).is_none() {
                return Err(QueryError::Abi(format!("missing required export `{export}`")));
            }
        }
        Ok(())
    }

    /// Run a Database Function over `input` with `ctx` under `limits`. Reads come
    /// from the pinned snapshot (+ the run's own writes); writes buffer into the
    /// ctx, returned for the caller to apply **once** on success. The attestation's
    /// `input_digest` is `hash(input ‖ snapshot_hash)` so corroboration binds to
    /// the exact data the run saw.
    pub fn run_with_ctx(
        &self,
        module_wasm: &[u8],
        input: &[u8],
        limits: &RunLimits,
        ctx: FunctionCtx,
    ) -> QueryResult<FunctionOutcome> {
        let workload_hash = hash(module_wasm);
        let input_digest = {
            let mut h = blake3::Hasher::new();
            h.update(input);
            h.update(&ctx.snapshot_hash());
            *h.finalize().as_bytes()
        };

        let module = self.module(&workload_hash, module_wasm)?;

        let max_bytes = limits.max_mem_mb as usize * 1024 * 1024;
        let mut store = Store::new(
            self.engine(),
            CtxHostState {
                limits: StoreLimitsBuilder::new().memory_size(max_bytes).build(),
                ctx,
                memory: None,
                alloc: None,
                resp: Vec::new(),
            },
        );
        store.limiter(|h| &mut h.limits);
        store.set_fuel(limits.max_fuel).map_err(|e| QueryError::Fuel(e.to_string()))?;

        let mut linker: Linker<CtxHostState> = Linker::new(self.engine());
        linker
            .func_wrap(
                "spacedb",
                "host_call",
                |mut caller: Caller<'_, CtxHostState>,
                 op_ptr: i32,
                 op_len: i32,
                 pay_ptr: i32,
                 pay_len: i32|
                 -> wasmtime::Result<i64> {
                    let memory = match caller.data().memory {
                        Some(memory) => memory,
                        None => caller
                            .get_export("memory")
                            .and_then(|e| e.into_memory())
                            .ok_or_else(|| wasmtime::Error::msg("host_call: no guest memory"))?,
                    };

                    // 1. Borrow op + payload straight out of guest memory and
                    // 2. dispatch — pure over (snapshot, overlay, op, payload).
                    // `data_and_store_mut` hands out the guest's bytes and the
                    // host state together (disjoint borrows), so neither the op
                    // nor the payload is copied out first.
                    let resp = {
                        let (data, state) = memory.data_and_store_mut(&mut caller);
                        let data: &[u8] = data;
                        let slice = |ptr: i32, len: i32| {
                            let (ptr, len) = (ptr as usize, len as usize);
                            data.get(ptr..ptr + len).ok_or_else(|| {
                                wasmtime::Error::msg("host_call: slice out of bounds")
                            })
                        };
                        let op_bytes = slice(op_ptr, op_len)?;
                        let payload = slice(pay_ptr, pay_len)?;
                        let op = std::str::from_utf8(op_bytes)
                            .map_err(|_| wasmtime::Error::msg("host_call: op must be UTF-8"))?;
                        let mut resp = std::mem::take(&mut state.resp);
                        state
                            .ctx
                            .dispatch(op, payload, &mut resp)
                            .map_err(wasmtime::Error::msg)?;
                        resp
                    };

                    // 3. Hand the response back through the guest's own allocator
                    //    (a reentrant call — its fuel burns deterministically).
                    let alloc: TypedFunc<i32, i32> = match caller.data().alloc.clone() {
                        Some(alloc) => alloc,
                        None => caller
                            .get_export("alloc")
                            .and_then(|e| e.into_func())
                            .ok_or_else(|| wasmtime::Error::msg("host_call: no guest alloc"))?
                            .typed(&caller)
                            .map_err(|e| wasmtime::Error::msg(format!("host_call: alloc: {e}")))?,
                    };
                    let len = i32::try_from(resp.len())
                        .map_err(|_| wasmtime::Error::msg("host_call: response too large"))?;
                    let ptr = alloc.call(&mut caller, len)?;
                    memory.write(&mut caller, ptr as usize, &resp)?;
                    let packed = (((ptr as u32) as i64) << 32) | (resp.len() as u32) as i64;
                    caller.data_mut().resp = resp; // keep the buffer for the next call
                    Ok(packed)
                },
            )
            .map_err(|e| QueryError::Instantiate(e.to_string()))?;

        let instance: Instance = linker
            .instantiate(&mut store, &module)
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
        store.data_mut().memory = Some(memory);
        store.data_mut().alloc = Some(alloc.clone());

        let in_len =
            i32::try_from(input.len()).map_err(|_| QueryError::Abi("input too large".into()))?;
        let in_ptr = alloc.call(&mut store, in_len).map_err(|e| QueryError::Trap(e.to_string()))?;
        memory
            .write(&mut store, in_ptr as usize, input)
            .map_err(|e| QueryError::Abi(format!("input write: {e}")))?;

        let packed =
            run.call(&mut store, (in_ptr, in_len)).map_err(|e| QueryError::Trap(e.to_string()))?;
        let packed = packed as u64;
        let (out_ptr, out_len) = ((packed >> 32) as usize, (packed & 0xFFFF_FFFF) as usize);
        let output = read_output(memory.data(&store), out_ptr, out_len)?;
        let output_digest = hash(&output);

        let remaining = store.get_fuel().map_err(|e| QueryError::Fuel(e.to_string()))?;
        let fuel_used = limits.max_fuel.saturating_sub(remaining);
        let mem_peak_mb = (memory.data_size(&store) / (1024 * 1024)) as u32;

        Ok(FunctionOutcome {
            execution: Execution {
                output,
                run: FunctionRun {
                    workload_hash,
                    input_digest,
                    output_digest,
                    fuel_used,
                    mem_peak_mb,
                },
            },
            ctx: store.into_data().ctx,
        })
    }
}

// ── framing helpers ────────────────────────────────────────────────────────────

/// Copy the guest's `(out_ptr, out_len)` result slice out of its memory — one
/// copy, where a zeroed buffer filled by `Memory::read` was a memset and a copy.
pub(crate) fn read_output(data: &[u8], out_ptr: usize, out_len: usize) -> QueryResult<Vec<u8>> {
    out_ptr
        .checked_add(out_len)
        .and_then(|end| data.get(out_ptr..end))
        .map(<[u8]>::to_vec)
        .ok_or_else(|| {
            // `Memory::read`'s error, word for word.
            QueryError::Abi(format!(
                "output read at {out_ptr}+{out_len}: out of bounds memory access"
            ))
        })
}

/// `collection \0 id` (both NUL-free UTF-8).
fn split2(payload: &[u8]) -> Option<(&str, &str)> {
    let nul = payload.iter().position(|b| *b == 0)?;
    let c = std::str::from_utf8(&payload[..nul]).ok()?;
    let id = std::str::from_utf8(&payload[nul + 1..]).ok()?;
    (!id.as_bytes().contains(&0)).then_some((c, id))
}

/// `collection \0 id \0 value` (value = arbitrary bytes, so it goes last).
fn split3(payload: &[u8]) -> Option<(&str, &str, &[u8])> {
    let first = payload.iter().position(|b| *b == 0)?;
    let rest = &payload[first + 1..];
    let second = rest.iter().position(|b| *b == 0)?;
    let c = std::str::from_utf8(&payload[..first]).ok()?;
    let id = std::str::from_utf8(&rest[..second]).ok()?;
    Some((c, id, &rest[second + 1..]))
}

fn frame(h: &mut blake3::Hasher, bytes: &[u8]) {
    h.update(&(bytes.len() as u32).to_le_bytes());
    h.update(bytes);
}

fn frame_into(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

fn hash_records(records: &RecordSnapshot) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    for ((c, id), v) in records {
        frame(&mut h, c.as_bytes());
        frame(&mut h, id.as_bytes());
        frame(&mut h, v);
    }
    *h.finalize().as_bytes()
}

fn hash(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}
