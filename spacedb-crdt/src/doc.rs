//! [`CrdtDoc`] — a convergent document with a typed field→CRDT-type mapping.
//!
//! A document is a yrs `Doc` (the Y-CRDT engine that guarantees convergence). On
//! top of it we expose a small set of **field types**, each chosen by the schema
//! for the merge behaviour it wants — this typed mapping is the genuinely new
//! work of M2; yrs handles the hard part (conflict-free merge).
//!
//! S1 ships two field types with deliberately different merge semantics:
//!
//! - **LWW-Register** — a scalar whose concurrent writes resolve last-writer-wins
//!   by the CRDT's logical clock (not wall time). The ~95% case: names, statuses,
//!   any "the latest value wins" field. Stored as a JSON string in a yrs map, so a
//!   register can hold any `serde` type.
//! - **PN-Counter** — a tally that merges by **summation**: each actor keeps its
//!   own running subtotal under its own key, and the value is the sum. Concurrent
//!   increments from different actors don't conflict; they add up. (Counters give
//!   a convergent *total*, not a non-negative invariant — that's a strong-tier,
//!   L3 concern.)
//!
//! ## Local-first
//!
//! Every mutation applies to the in-memory document and returns immediately —
//! there is no coordination on the write path. Changes are shipped as **updates**
//! ([`CrdtDoc::encode_update_since`]) and merged on other replicas
//! ([`CrdtDoc::apply_update`]); the merge is order-independent (the convergence
//! property test proves it). Persistence into the encrypted store is M2-S2.
//!
//! ## Actor id
//!
//! Each replica has an **actor id** (the yrs client id) — in production derived
//! from the writer's device/mID key so provenance is intrinsic. Two replicas must
//! not share an actor id.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{de::DeserializeOwned, Serialize};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{
    Array, Doc, GetString, Map, MapRef, ReadTxn, StateVector, Subscription, Text, Transact, Update,
    WriteTxn,
};

use crate::error::{CrdtError, CrdtResult};
use crate::reactive::Watcher;

/// The single root map that holds register values and per-actor counter subtotals.
const FIELDS_MAP: &str = "_fields";

/// Separator that namespaces counter subtotal keys away from register keys.
/// `0x01` is not expected in field names, so `register("x")` (key `"x"`) and
/// `counter("x")` (keys `"\u{1}c\u{1}x\u{1}<actor>"`) never collide.
const SEP: char = '\u{1}';

/// A convergent document: a yrs CRDT plus a typed field API.
pub struct CrdtDoc {
    doc: Doc,
    fields: MapRef,
    actor: u64,
    /// Bumped on every mutating transaction (local or remote-applied) by the
    /// update observer below — the basis for reactive queries.
    revision: Arc<AtomicU64>,
    /// The raw v1 update bytes of every **local** mutation, buffered for broadcast.
    /// Shipping these verbatim (rather than re-encoding a delta or a snapshot) is the
    /// canonical, merge-defect-free way to replicate: each atomic update is applied
    /// exactly once on every peer, so there is no partial-overlap re-merge that the
    /// underlying engine mishandles for some actor-id orderings. Drained by
    /// [`CrdtDoc::take_local_updates`].
    pending: Arc<Mutex<Vec<Vec<u8>>>>,
    /// True only while [`CrdtDoc::apply_update`] is integrating a *remote* update, so
    /// the observer can tell remote applies (don't re-buffer them) from local writes.
    applying_remote: Arc<AtomicBool>,
    /// Keeps the revision-bumping observer registered for the doc's lifetime.
    _update_sub: Subscription,
}

impl CrdtDoc {
    /// Create a document for the replica identified by `actor_id` (the yrs client
    /// id). Two replicas must use distinct ids.
    pub fn new(actor_id: u64) -> Self {
        let doc = Doc::with_client_id(actor_id);
        // Create the root field map before attaching the observer, so its
        // one-time creation isn't counted as a content change.
        let fields = doc.transact_mut().get_or_insert_map(FIELDS_MAP);

        let revision = Arc::new(AtomicU64::new(0));
        let pending = Arc::new(Mutex::new(Vec::new()));
        let applying_remote = Arc::new(AtomicBool::new(false));
        let revision_for_obs = Arc::clone(&revision);
        let pending_for_obs = Arc::clone(&pending);
        let remote_for_obs = Arc::clone(&applying_remote);
        let update_sub = doc
            .observe_update_v1(move |_txn, event| {
                revision_for_obs.fetch_add(1, Ordering::Relaxed);
                // Buffer this update for broadcast only if it's a local mutation; a
                // remote apply (flag set) is already in the log we pulled it from.
                if !remote_for_obs.load(Ordering::Relaxed) {
                    pending_for_obs.lock().unwrap().push(event.update.clone());
                }
            })
            .expect("no transaction is active during construction");

        Self {
            doc,
            fields,
            actor: actor_id,
            revision,
            pending,
            applying_remote,
            _update_sub: update_sub,
        }
    }

    /// Drain the raw v1 update bytes of every local mutation since the last drain —
    /// the deltas to broadcast to peers / append to the sync log. Each is applied
    /// verbatim (and idempotently) by every other replica.
    pub fn take_local_updates(&self) -> Vec<Vec<u8>> {
        std::mem::take(&mut *self.pending.lock().unwrap())
    }

    /// This replica's actor id.
    pub fn actor_id(&self) -> u64 {
        self.actor
    }

    /// A monotonic revision counter, bumped on every change to the document
    /// (local mutation or applied remote update). Reads never bump it.
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }

    /// Start watching this document for changes — the basis for reactive queries.
    /// The returned [`Watcher`] owns a handle to the revision counter, so it is
    /// independent of any borrow of the document.
    pub fn watch(&self) -> Watcher {
        Watcher::new(Arc::clone(&self.revision))
    }

    // ─── LWW-Register ────────────────────────────────────────────────────────

    /// Set a last-writer-wins register field to `value`.
    pub fn set_register<T: Serialize>(&self, field: &str, value: &T) -> CrdtResult<()> {
        let json = serde_json::to_string(value).map_err(|source| CrdtError::ValueCodec {
            field: field.to_string(),
            source,
        })?;
        let mut txn = self.doc.transact_mut();
        self.fields.insert(&mut txn, field, json);
        Ok(())
    }

    /// Every register field currently set — the keys of the single root map (internal
    /// counter keys excluded). Unlike a per-field set (a separate root array), the one
    /// root map merges cleanly across replicas through any relay, so enumerating
    /// records this way is convergence-safe.
    pub fn register_keys(&self) -> Vec<String> {
        let txn = self.doc.transact();
        self.fields
            .iter(&txn)
            .map(|(k, _)| k.to_string())
            .filter(|k| !k.starts_with(SEP))
            .collect()
    }

    /// Clear a register field (remove it from the root map).
    pub fn remove_register(&self, field: &str) {
        let mut txn = self.doc.transact_mut();
        self.fields.remove(&mut txn, field);
    }

    /// Read a register field, or `None` if it was never set.
    pub fn get_register<T: DeserializeOwned>(&self, field: &str) -> CrdtResult<Option<T>> {
        let txn = self.doc.transact();
        match self.fields.get(&txn, field) {
            None => Ok(None),
            Some(out) => {
                let json = out.to_string(&txn);
                let value = serde_json::from_str(&json).map_err(|source| CrdtError::ValueCodec {
                    field: field.to_string(),
                    source,
                })?;
                Ok(Some(value))
            }
        }
    }

    // ─── PN-Counter ──────────────────────────────────────────────────────────

    fn counter_key(field: &str, actor: u64) -> String {
        format!("{SEP}c{SEP}{field}{SEP}{actor}")
    }

    fn counter_prefix(field: &str) -> String {
        format!("{SEP}c{SEP}{field}{SEP}")
    }

    /// Add `delta` (which may be negative) to a PN-counter field. Each actor
    /// accumulates into its own subtotal, so concurrent increments merge by sum.
    pub fn increment(&self, field: &str, delta: i64) {
        let key = Self::counter_key(field, self.actor);
        let mut txn = self.doc.transact_mut();
        let current: i64 = match self.fields.get(&txn, &key) {
            Some(out) => out.to_string(&txn).parse().unwrap_or(0),
            None => 0,
        };
        // Stored as a decimal string (everything in the map is a string), so the
        // value codec stays uniform with registers.
        self.fields.insert(&mut txn, key, (current + delta).to_string());
    }

    /// The current value of a PN-counter field: the sum of every actor's subtotal.
    pub fn counter(&self, field: &str) -> i64 {
        let prefix = Self::counter_prefix(field);
        let txn = self.doc.transact();
        self.fields
            .iter(&txn)
            .filter(|(k, _)| k.starts_with(&prefix))
            .map(|(_, v)| v.to_string(&txn).parse::<i64>().unwrap_or(0))
            .sum()
    }

    // ─── Y.Text (collaborative sequence) ─────────────────────────────────────

    fn text_name(field: &str) -> String {
        format!("text{SEP}{field}")
    }

    /// Append `content` to a collaborative text field.
    pub fn text_push(&self, field: &str, content: &str) {
        let mut txn = self.doc.transact_mut();
        let text = txn.get_or_insert_text(Self::text_name(field).as_str());
        text.push(&mut txn, content);
    }

    /// Insert `content` at character `index` in a collaborative text field.
    pub fn text_insert(&self, field: &str, index: u32, content: &str) {
        let mut txn = self.doc.transact_mut();
        let text = txn.get_or_insert_text(Self::text_name(field).as_str());
        text.insert(&mut txn, index, content);
    }

    /// Remove `len` characters starting at `index` from a collaborative text
    /// field. The removed run becomes a tombstone (yrs handles convergence).
    pub fn text_remove(&self, field: &str, index: u32, len: u32) {
        let mut txn = self.doc.transact_mut();
        let text = txn.get_or_insert_text(Self::text_name(field).as_str());
        text.remove_range(&mut txn, index, len);
    }

    /// The current contents of a collaborative text field (empty if never set).
    pub fn text(&self, field: &str) -> String {
        let txn = self.doc.transact();
        match txn.get_text(Self::text_name(field).as_str()) {
            Some(t) => t.get_string(&txn),
            None => String::new(),
        }
    }

    /// The character length of a collaborative text field.
    pub fn text_len(&self, field: &str) -> u32 {
        let txn = self.doc.transact();
        txn.get_text(Self::text_name(field).as_str())
            .map(|t| t.len(&txn))
            .unwrap_or(0)
    }

    // ─── OR-Set (add-wins, observed-remove) ──────────────────────────────────
    //
    // Backed by a yrs Array used as an add-log: each `add` appends the element
    // (yrs gives every insert a unique block id), and `remove` deletes the
    // occurrences this replica currently observes. A concurrent add the remover
    // never saw is a different block, so it survives — add-wins. Tombstones for
    // removed occurrences are yrs's job.

    fn set_name(field: &str) -> String {
        format!("set{SEP}{field}")
    }

    /// Add `element` to an OR-Set field.
    pub fn set_add(&self, field: &str, element: &str) {
        let mut txn = self.doc.transact_mut();
        let arr = txn.get_or_insert_array(Self::set_name(field).as_str());
        arr.push_back(&mut txn, element.to_string());
    }

    /// Remove every currently-observed occurrence of `element` from an OR-Set
    /// field. A concurrent add this replica hasn't seen survives (add-wins).
    pub fn set_remove(&self, field: &str, element: &str) {
        let mut txn = self.doc.transact_mut();
        let arr = txn.get_or_insert_array(Self::set_name(field).as_str());
        // Collect matching indices, then delete from the back so earlier indices
        // stay valid as we remove.
        let mut matches = Vec::new();
        for (i, out) in arr.iter(&txn).enumerate() {
            if out.to_string(&txn) == element {
                matches.push(i as u32);
            }
        }
        for &i in matches.iter().rev() {
            arr.remove_range(&mut txn, i, 1);
        }
    }

    /// Whether `element` is currently in an OR-Set field.
    pub fn set_contains(&self, field: &str, element: &str) -> bool {
        let txn = self.doc.transact();
        match txn.get_array(Self::set_name(field).as_str()) {
            Some(arr) => arr.iter(&txn).any(|out| out.to_string(&txn) == element),
            None => false,
        }
    }

    /// The members of an OR-Set field, deduplicated and sorted.
    pub fn set_members(&self, field: &str) -> Vec<String> {
        let txn = self.doc.transact();
        let mut members = std::collections::BTreeSet::new();
        if let Some(arr) = txn.get_array(Self::set_name(field).as_str()) {
            for out in arr.iter(&txn) {
                members.insert(out.to_string(&txn));
            }
        }
        members.into_iter().collect()
    }

    // ─── compaction & size ───────────────────────────────────────────────────

    /// The size, in bytes, of this document's full-state encoding — exactly what
    /// a fresh peer would have to receive. Use it as a **size advisory**: sequence
    /// fields (Y.Text, OR-Set) carry the most per-item metadata, so a very large
    /// ordered collection grows this faster than the same data in registers.
    ///
    /// ## Compaction posture
    ///
    /// yrs garbage collection is **on by default**, so deleted *content* (e.g. the
    /// bytes of removed text) is collected automatically — `text_remove` of a
    /// large run shrinks this number. What remains are small structural deletion
    /// markers, kept so convergence is safe. Pruning *those* markers — the
    /// "drop causally-stable history once every replica has acked a frontier"
    /// compaction — requires the universally-acked-frontier protocol and is
    /// deferred to a later phase (the mission flags production compaction hardening
    /// as Phase 2). We do not fake it here.
    pub fn estimated_state_size(&self) -> usize {
        self.encode_full().len()
    }

    // ─── sync primitives ─────────────────────────────────────────────────────

    /// This document's state vector (the per-actor version frontier), v1-encoded.
    /// A peer sends this to ask "what have I not seen?"
    pub fn state_vector(&self) -> Vec<u8> {
        self.doc.transact().state_vector().encode_v1()
    }

    /// Encode the updates this document has that a peer at `their_state_vector`
    /// does not — the delta to bring that peer up to date (anti-entropy).
    pub fn encode_update_since(&self, their_state_vector: &[u8]) -> CrdtResult<Vec<u8>> {
        let sv = StateVector::decode_v1(their_state_vector)
            .map_err(|e| CrdtError::DecodeStateVector(e.to_string()))?;
        Ok(self.doc.transact().encode_state_as_update_v1(&sv))
    }

    /// Encode the document's entire state as a single update (the delta from
    /// empty) — used to seed a fresh replica.
    pub fn encode_full(&self) -> Vec<u8> {
        self.doc
            .transact()
            .encode_state_as_update_v1(&StateVector::default())
    }

    /// Merge a remote update into this document. Conflict-free and
    /// order-independent: applying the same set of updates in any order converges
    /// to the same state.
    pub fn apply_update(&self, update: &[u8]) -> CrdtResult<()> {
        let update =
            Update::decode_v1(update).map_err(|e| CrdtError::DecodeUpdate(e.to_string()))?;
        // Mark this as a remote integration so the observer doesn't re-buffer it as a
        // local update. The flag must stay set until the transaction commits (on drop),
        // because that's when the observer fires.
        self.applying_remote.store(true, Ordering::Relaxed);
        let result = {
            let mut txn = self.doc.transact_mut();
            txn.apply_update(update)
                .map_err(|e| CrdtError::ApplyUpdate(e.to_string()))
        };
        self.applying_remote.store(false, Ordering::Relaxed);
        result
    }

    /// **Convergence lag**: how many operations a peer (described by its
    /// `peer_state_vector`) has that this replica has not yet seen. Zero means
    /// this replica is caught up to the peer's announced frontier. The replica
    /// layer uses this to report honest read freshness (`Live` vs `Stale{lag}`).
    pub fn ops_behind(&self, peer_state_vector: &[u8]) -> CrdtResult<usize> {
        let peer = StateVector::decode_v1(peer_state_vector)
            .map_err(|e| CrdtError::DecodeStateVector(e.to_string()))?;
        let txn = self.doc.transact();
        let mine = txn.state_vector();
        let mut missing = 0usize;
        for (client, peer_clock) in peer.iter() {
            let my_clock = mine.get(client);
            if *peer_clock > my_clock {
                missing += (*peer_clock - my_clock) as usize;
            }
        }
        Ok(missing)
    }
}
