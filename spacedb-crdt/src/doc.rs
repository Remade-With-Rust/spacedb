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
//! Each replica has an **actor id** — in production derived from the writer's
//! device/mID key so provenance is intrinsic. Two replicas must not share an
//! actor id.
//!
//! It is **not** the yrs client id, and the two must not be bound together. A
//! yrs client id identifies a document *instance*: the clock is per client and
//! a freshly constructed `Doc` starts at zero, so a reloaded document that
//! reuses its actor id as its client id writes over clocks its own imported
//! blocks already hold. See [`CrdtDoc::new`].

use std::borrow::Cow;
use std::fmt::{self, Write as _};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{de::DeserializeOwned, Serialize};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{
    Any, Array, Doc, GetString, Map, MapRef, Out, ReadTxn, StateVector, Text, Transact,
    TransactionMut, Update, WriteTxn,
};

use crate::error::{CrdtError, CrdtResult};
use crate::reactive::Watcher;

/// The single root map that holds register values and per-actor counter subtotals.
const FIELDS_MAP: &str = "_fields";

/// Separator that namespaces counter subtotal keys away from register keys.
/// `0x01` is not expected in field names, so `register("x")` (key `"x"`) and
/// `counter("x")` (keys `"\u{1}c\u{1}x\u{1}<actor>"`) never collide.
const SEP: char = '\u{1}';

/// Run `f` on a stored value's text. Everything this document writes is a string
/// (`Any::String`), whose text is borrowed rather than copied; anything else
/// falls back to `Out::to_string`, exactly what the callers did before.
fn with_text<R, T: ReadTxn>(out: Out, txn: &T, f: impl FnOnce(&str) -> R) -> R {
    match out {
        Out::Any(Any::String(s)) => f(&s),
        other => f(&other.to_string(txn)),
    }
}

/// A short string formatted into a fixed stack buffer - writing past `N` bytes
/// fails rather than allocating, so the caller can fall back to a `String`.
struct StackStr<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> StackStr<N> {
    fn new() -> Self {
        Self { buf: [0; N], len: 0 }
    }

    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.buf[..self.len]).expect("written only from &str")
    }
}

impl<const N: usize> fmt::Write for StackStr<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.len + s.len();
        let dst = self.buf.get_mut(self.len..end).ok_or(fmt::Error)?;
        dst.copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }

    // The default goes through `encode_utf8` and `write_str`: a runtime-length
    // copy of one byte for every separator.
    fn write_char(&mut self, c: char) -> fmt::Result {
        if c.is_ascii() {
            *self.buf.get_mut(self.len).ok_or(fmt::Error)? = c as u8;
            self.len += 1;
            Ok(())
        } else {
            self.write_str(c.encode_utf8(&mut [0; 4]))
        }
    }
}

impl<const N: usize> StackStr<N> {
    /// Append the decimal form of `v`, the way `write!("{v}")` would, without
    /// the formatter: a `Display` round trip through `fmt::write` and
    /// `pad_integral` was a third of every counter increment's self cost.
    #[inline]
    fn push_i64(&mut self, v: i64) -> fmt::Result {
        if v < 0 {
            self.write_char('-')?;
        }
        self.push_u64(v.unsigned_abs())
    }

    #[inline]
    fn push_u64(&mut self, mut v: u64) -> fmt::Result {
        let mut digits = [0u8; 20];
        let mut at = digits.len();
        loop {
            at -= 1;
            digits[at] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        let text = &digits[at..];
        if self.len + text.len() > N {
            return Err(fmt::Error);
        }
        self.buf[self.len..self.len + text.len()].copy_from_slice(text);
        self.len += text.len();
        Ok(())
    }

    /// `write_str` for the three one-byte separators and the field name: the
    /// same bytes `write!` produces, by direct copy.
    #[inline]
    fn push_counter_key(&mut self, field: &str, actor: u64) -> fmt::Result {
        self.write_char(SEP)?;
        self.write_char('c')?;
        self.write_char(SEP)?;
        self.write_str(field)?;
        self.write_char(SEP)?;
        self.push_u64(actor)
    }
}

/// A derived root/key name (`text<SEP>field`, ...), formatted on the stack when
/// it fits - it is only ever looked up by `&str` - and on the heap when not.
enum Name {
    Stack(StackStr<96>),
    Heap(String),
}

impl Name {
    fn new(args: fmt::Arguments<'_>) -> Self {
        let mut s = StackStr::new();
        if s.write_fmt(args).is_ok() {
            Name::Stack(s)
        } else {
            Name::Heap(fmt::format(args))
        }
    }

    fn as_str(&self) -> &str {
        match self {
            Name::Stack(s) => s.as_str(),
            Name::Heap(s) => s,
        }
    }
}

/// A convergent document: a yrs CRDT plus a typed field API.
pub struct CrdtDoc {
    doc: Doc,
    fields: MapRef,
    actor: u64,
    /// Bumped on every mutating transaction (local or remote-applied) by
    /// [`finish`](Self::finish) — the basis for reactive queries.
    revision: Arc<AtomicU64>,
    /// The raw v1 update bytes of every **local** mutation, buffered for broadcast.
    /// Shipping these verbatim (rather than re-encoding a delta or a snapshot) is the
    /// canonical, merge-defect-free way to replicate: each atomic update is applied
    /// exactly once on every peer, so there is no partial-overlap re-merge that the
    /// underlying engine mishandles for some actor-id orderings. Drained by
    /// [`CrdtDoc::take_local_updates`].
    pending: Mutex<Vec<Vec<u8>>>,
    /// Whether local mutations are buffered into `pending` at all (see
    /// [`new_unlogged`](Self::new_unlogged)).
    log_local_updates: bool,
    /// The last [`state_vector`](Self::state_vector) encoding and the revision
    /// it was taken at: the vector only changes when the document does, and
    /// every change bumps the revision.
    sv_cache: Mutex<Option<(u64, Vec<u8>)>>,
    /// Recent [`counter`](Self::counter) totals as `(revision, field, total)`:
    /// a total only changes when the document does, and every change bumps the
    /// revision, so a hit skips the walk over every key and the parse of every
    /// subtotal. A few slots cover the fields a caller polls between writes.
    counter_memo: Mutex<Vec<(u64, String, i64)>>,
    /// The members of the last OR-Set field queried by
    /// [`set_contains`](Self::set_contains), as `(revision, field, members)`:
    /// rebuilt when the document changes, so repeated membership tests are a
    /// hash lookup instead of a string compare against every element.
    set_memo: Mutex<Option<(u64, String, std::collections::HashSet<String>)>>,
    /// The last delta [`encode_update_since`](Self::encode_update_since)
    /// produced, as `(revision, peer state vector, update)`. A delta is a pure
    /// function of the document's state and the peer's vector: while neither
    /// moves, the same bytes come back from here instead of a walk over every
    /// block of the store (the delete set is re-derived from all of them on
    /// each encode). Anti-entropy rounds between converged replicas ask the
    /// same question again and again.
    delta_memo: Mutex<Option<(u64, Vec<u8>, Vec<u8>)>>,
}

/// Deltas longer than this are not remembered (one buffer per document).
const DELTA_MEMO_MAX: usize = 256 * 1024;

/// How many counter fields the read memo remembers.
const COUNTER_MEMO_SLOTS: usize = 4;

impl CrdtDoc {
    /// Create a document for the replica identified by `actor_id` (the yrs client
    /// id). Two replicas must use distinct ids.
    pub fn new(actor_id: u64) -> Self {
        Self::with_update_log(actor_id, true)
    }

    /// [`new`](Self::new) for a replica that never relays its local updates
    /// verbatim - one that syncs only by state-vector anti-entropy
    /// ([`state_vector`](Self::state_vector) / [`encode_update_since`](Self::encode_update_since))
    /// or by full state. Its local writes skip encoding an update nobody will
    /// drain (and the buffer that would otherwise grow for the document's life);
    /// [`take_local_updates`](Self::take_local_updates) always returns nothing.
    /// Everything else, the revision counter included, behaves as for `new`.
    pub fn new_unlogged(actor_id: u64) -> Self {
        Self::with_update_log(actor_id, false)
    }

    fn with_update_log(actor_id: u64, log_local_updates: bool) -> Self {
        // A FRESH yrs client id, deliberately not `actor_id`.
        //
        // These are two different things and binding them together loses data.
        // A yrs client id identifies a document INSTANCE, because the clock is
        // per client and a newly constructed `Doc` starts its clock at zero.
        // An actor id identifies a replica across its whole life.
        //
        // Reuse the actor id as the client id and every reload is a new
        // document claiming to be the same client with a rewound clock: the
        // blocks it imports occupy clocks 0..n for that client, and the first
        // local write afterwards takes a clock already spoken for. The document
        // that results cannot be imported again - measured, it fails on the
        // third load, and `yrs` panics with a divide-by-zero in `find_pivot`
        // rather than reporting anything.
        //
        // Fresh-id-per-instance is the ordinary way to load a persisted yrs
        // document: prior blocks keep the client id they were written under and
        // new writes get their own, which is what makes the merge well-defined.
        //
        // `actor_id` is still the replica's identity and still what counter
        // subtotals are keyed by - see `counter_key` - so provenance and the
        // "two replicas must not share an actor id" invariant are unchanged.
        let doc = Doc::new();
        // Create the root field map before attaching the observer, so its
        // one-time creation isn't counted as a content change.
        let fields = doc.transact_mut().get_or_insert_map(FIELDS_MAP);

        // No yrs observer. Every transaction on this document is opened by a
        // method of this type, and each of them ends in `finish`, which does
        // what the transaction-cleanup observer used to: the "anything
        // changed" test, the revision bump and, for a logged document, the
        // raw-update encoding of a local mutation. The observer cost every
        // commit a cloned before-state, after-state and delete set (for the
        // event it was handed) plus an atomic flag to tell local from remote;
        // here the transaction is read in place and locality is known at the
        // call site.
        Self {
            doc,
            fields,
            actor: actor_id,
            revision: Arc::new(AtomicU64::new(0)),
            pending: Mutex::new(Vec::new()),
            log_local_updates,
            sv_cache: Mutex::new(None),
            counter_memo: Mutex::new(Vec::new()),
            set_memo: Mutex::new(None),
            delta_memo: Mutex::new(None),
        }
    }

    /// Commit `txn` and account for it: a transaction that changed nothing
    /// (no block integrated, nothing deleted) leaves the document as it was;
    /// one that did bumps the revision and, when it was a `local` mutation on
    /// a logged document, buffers its raw update for broadcast. A remote apply
    /// is already in the log it came from. `encode_update_v1` after the commit
    /// is byte-for-byte what yrs itself would have encoded for an update
    /// observer: the same store, after the same block squashing.
    fn finish(&self, txn: &mut TransactionMut, local: bool) {
        txn.commit();
        if txn.delete_set().is_empty() && txn.after_state() == txn.before_state() {
            return;
        }
        self.revision.fetch_add(1, Ordering::Relaxed);
        if local && self.log_local_updates {
            self.pending.lock().unwrap().push(txn.encode_update_v1());
        }
    }

    /// [`finish`](Self::finish) for a local mutation KNOWN to have integrated
    /// a block - a map insert, an array push, a non-empty text insert: yrs
    /// creates an item for each of these unconditionally, so the after-state
    /// has moved and the comparison of the two state vectors (a hash lookup
    /// per client the document has ever seen) is skipped.
    fn finish_changed(&self, txn: &mut TransactionMut) {
        txn.commit();
        self.revision.fetch_add(1, Ordering::Relaxed);
        if self.log_local_updates {
            self.pending.lock().unwrap().push(txn.encode_update_v1());
        }
    }

    /// Drain the raw v1 update bytes of every local mutation since the last drain —
    /// the deltas to broadcast to peers / append to the sync log. Each is applied
    /// verbatim (and idempotently) by every other replica.
    pub fn take_local_updates(&self) -> Vec<Vec<u8>> {
        std::mem::take(&mut *self.pending.lock().unwrap())
    }

    /// This replica's actor id. Not the yrs client id - see [`CrdtDoc::new`].
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
        let mut txn = self.doc.transact_mut();
        let result = self.set_register_in(&mut txn, field, value);
        // A value that failed to encode inserted nothing.
        if result.is_ok() {
            self.finish_changed(&mut txn);
        } else {
            self.finish(&mut txn, true);
        }
        result
    }

    // The body of the per-op method and of its batch twin; one caller is hot,
    // so it is inlined into both rather than paying a frame at each.
    #[inline(always)]
    fn set_register_in<T: Serialize>(
        &self,
        txn: &mut TransactionMut,
        field: &str,
        value: &T,
    ) -> CrdtResult<()> {
        let json = serde_json::to_string(value).map_err(|source| CrdtError::ValueCodec {
            field: field.to_string(),
            source,
        })?;
        self.fields.insert(txn, field, json);
        Ok(())
    }

    // ─── batches ─────────────────────────────────────────────────────────────

    /// Run several mutations in ONE yrs transaction. Each mutator on this
    /// document opens and commits its own transaction, and the commit - the
    /// observer event, the revision bump and (for a logged document) the update
    /// encoding - is most of a small write's cost. A batch pays it once: the
    /// converged state is identical, and a logged document buffers one update
    /// for the whole batch instead of one per mutation.
    pub fn batch<R>(&self, f: impl FnOnce(&mut Batch<'_>) -> R) -> R {
        let txn = self.doc.transact_mut();
        let mut batch = Batch { doc: self, txn };
        let result = f(&mut batch);
        self.finish(&mut batch.txn, true);
        result
    }

    /// Every register field currently set — the keys of the single root map (internal
    /// counter keys excluded). Unlike a per-field set (a separate root array), the one
    /// root map merges cleanly across replicas through any relay, so enumerating
    /// records this way is convergence-safe.
    pub fn register_keys(&self) -> Vec<String> {
        let txn = self.doc.transact();
        self.fields
            .iter(&txn)
            .filter(|(k, _)| !k.starts_with(SEP))
            .map(|(k, _)| k.to_string())
            .collect()
    }

    /// Clear a register field (remove it from the root map).
    pub fn remove_register(&self, field: &str) {
        let mut txn = self.doc.transact_mut();
        self.fields.remove(&mut txn, field);
        self.finish(&mut txn, true);
    }

    /// Read a register field, or `None` if it was never set.
    pub fn get_register<T: DeserializeOwned>(&self, field: &str) -> CrdtResult<Option<T>> {
        let txn = self.doc.transact();
        match self.fields.get(&txn, field) {
            None => Ok(None),
            Some(out) => {
                let value = with_text(out, &txn, |s| serde_json::from_str(s)).map_err(|source| {
                    CrdtError::ValueCodec {
                        field: field.to_string(),
                        source,
                    }
                })?;
                Ok(Some(value))
            }
        }
    }

    // ─── PN-Counter ──────────────────────────────────────────────────────────

    fn counter_key(field: &str, actor: u64) -> String {
        format!("{SEP}c{SEP}{field}{SEP}{actor}")
    }

    // Stays a `String`: `counter` runs this once per read over every key, and
    // the stack `Name` measured +523K Ir on the sim's convergence probes.
    fn counter_prefix(field: &str) -> String {
        format!("{SEP}c{SEP}{field}{SEP}")
    }

    /// Add `delta` (which may be negative) to a PN-counter field. Each actor
    /// accumulates into its own subtotal, so concurrent increments merge by sum.
    pub fn increment(&self, field: &str, delta: i64) {
        let mut txn = self.doc.transact_mut();
        self.increment_in(&mut txn, field, delta);
        self.finish_changed(&mut txn);
    }

    // The body of the per-op method and of its batch twin; one caller is hot,
    // so it is inlined into both rather than paying a frame at each.
    #[inline(always)]
    fn increment_in(&self, txn: &mut TransactionMut, field: &str, delta: i64) {
        let mut on_stack = StackStr::<96>::new();
        let key: Cow<str> = if on_stack.push_counter_key(field, self.actor).is_ok() {
            Cow::Borrowed(on_stack.as_str())
        } else {
            Cow::Owned(Self::counter_key(field, self.actor))
        };
        let current: i64 = match self.fields.get(txn, &key) {
            Some(out) => with_text(out, txn, |s| s.parse().unwrap_or(0)),
            None => 0,
        };
        // Stored as a decimal string (everything in the map is a string), so the
        // value codec stays uniform with registers. An i64 is at most 20 digits.
        let mut value = StackStr::<24>::new();
        value.push_i64(current + delta).expect("an i64 fits in 24 bytes");
        self.fields.insert(txn, &*key, value.as_str());
    }

    /// The current value of a PN-counter field: the sum of every actor's subtotal.
    pub fn counter(&self, field: &str) -> i64 {
        let revision = self.revision();
        let mut memo = self.counter_memo.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((_, _, total)) = memo
            .iter()
            .find(|(at, f, _)| *at == revision && f == field)
        {
            return *total;
        }
        let total = self.sum_counter(field);
        // Replace this field's stale entry, else the stalest entry, else append.
        if let Some(slot) = memo.iter_mut().find(|(_, f, _)| f == field) {
            *slot = (revision, std::mem::take(&mut slot.1), total);
        } else if memo.len() < COUNTER_MEMO_SLOTS {
            memo.push((revision, field.to_string(), total));
        } else if let Some(slot) = memo.iter_mut().min_by_key(|(at, _, _)| *at) {
            slot.0 = revision;
            slot.1.clear();
            slot.1.push_str(field);
            slot.2 = total;
        }
        total
    }

    /// The walk behind [`counter`](Self::counter): every subtotal under the field's prefix.
    fn sum_counter(&self, field: &str) -> i64 {
        let prefix = Self::counter_prefix(field);
        let txn = self.doc.transact();
        self.fields
            .iter(&txn)
            .filter(|(k, _)| k.starts_with(&prefix))
            .map(|(_, v)| with_text(v, &txn, |s| s.parse::<i64>().unwrap_or(0)))
            .sum()
    }

    // ─── Y.Text (collaborative sequence) ─────────────────────────────────────

    fn text_name(field: &str) -> Name {
        Name::new(format_args!("text{SEP}{field}"))
    }

    /// Append `content` to a collaborative text field.
    pub fn text_push(&self, field: &str, content: &str) {
        let mut txn = self.doc.transact_mut();
        Self::text_push_in(&mut txn, field, content);
        if content.is_empty() {
            self.finish(&mut txn, true);
        } else {
            self.finish_changed(&mut txn);
        }
    }

    // The body of the per-op method and of its batch twin; one caller is hot,
    // so it is inlined into both rather than paying a frame at each.
    #[inline(always)]
    fn text_push_in(txn: &mut TransactionMut, field: &str, content: &str) {
        let text = txn.get_or_insert_text(Self::text_name(field).as_str());
        text.push(txn, content);
    }

    /// Insert `content` at character `index` in a collaborative text field.
    pub fn text_insert(&self, field: &str, index: u32, content: &str) {
        let mut txn = self.doc.transact_mut();
        let text = txn.get_or_insert_text(Self::text_name(field).as_str());
        text.insert(&mut txn, index, content);
        self.finish(&mut txn, true);
    }

    /// Remove `len` characters starting at `index` from a collaborative text
    /// field. The removed run becomes a tombstone (yrs handles convergence).
    pub fn text_remove(&self, field: &str, index: u32, len: u32) {
        let mut txn = self.doc.transact_mut();
        let text = txn.get_or_insert_text(Self::text_name(field).as_str());
        text.remove_range(&mut txn, index, len);
        self.finish(&mut txn, true);
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

    fn set_name(field: &str) -> Name {
        Name::new(format_args!("set{SEP}{field}"))
    }

    /// Add `element` to an OR-Set field.
    pub fn set_add(&self, field: &str, element: &str) {
        let mut txn = self.doc.transact_mut();
        Self::set_add_in(&mut txn, field, element);
        self.finish_changed(&mut txn);
    }

    // The body of the per-op method and of its batch twin; one caller is hot,
    // so it is inlined into both rather than paying a frame at each.
    #[inline(always)]
    fn set_add_in(txn: &mut TransactionMut, field: &str, element: &str) {
        let arr = txn.get_or_insert_array(Self::set_name(field).as_str());
        // `&str` becomes the same `Any::String` a `String` would, minus the copy.
        arr.push_back(txn, element);
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
            if with_text(out, &txn, |s| s == element) {
                matches.push(i as u32);
            }
        }
        for &i in matches.iter().rev() {
            arr.remove_range(&mut txn, i, 1);
        }
        self.finish(&mut txn, true);
    }

    /// Whether `element` is currently in an OR-Set field.
    pub fn set_contains(&self, field: &str, element: &str) -> bool {
        let revision = self.revision();
        let mut memo = self.set_memo.lock().unwrap_or_else(|p| p.into_inner());
        match &*memo {
            Some((at, f, members)) if *at == revision && f == field => members.contains(element),
            _ => {
                let txn = self.doc.transact();
                let mut members = std::collections::HashSet::new();
                if let Some(arr) = txn.get_array(Self::set_name(field).as_str()) {
                    for out in arr.iter(&txn) {
                        members.insert(out.to_string(&txn));
                    }
                }
                let present = members.contains(element);
                *memo = Some((revision, field.to_string(), members));
                present
            }
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
        self.cached_state_vector(Vec::clone)
    }

    /// Whether `bytes` is exactly this document's current state-vector encoding
    /// (the bytes [`state_vector`](Self::state_vector) would return). Served from
    /// the cache: a session whose causal token is this document's own frontier
    /// learns it is caught up without decoding anything.
    pub fn state_vector_is(&self, bytes: &[u8]) -> bool {
        // Answered from the cache only: a stale cache means the document moved
        // since the vector was taken, and this says "no" without rebuilding it
        // (the caller's slow path will, in its own single transaction).
        let revision = self.revision();
        let cache = self.sv_cache.lock().unwrap_or_else(|p| p.into_inner());
        matches!(&*cache, Some((at, sv)) if *at == revision && sv.as_slice() == bytes)
    }

    /// [`state_vector`](Self::state_vector) written into `out` (cleared first),
    /// reusing its allocation.
    pub fn state_vector_into(&self, out: &mut Vec<u8>) {
        self.cached_state_vector(|sv| out.clone_from(sv))
    }

    /// `tag ‖ `[`state_vector`](Self::state_vector) in one allocation, for a
    /// sender that frames the vector behind a tag byte: it is copied once, not
    /// twice.
    pub fn tagged_state_vector(&self, tag: u8) -> Vec<u8> {
        self.cached_state_vector(|sv| {
            let mut out = Vec::with_capacity(1 + sv.len());
            out.push(tag);
            out.extend_from_slice(sv);
            out
        })
    }

    /// Runs `f` on the current state vector's encoding, re-encoding only if the
    /// document changed since the last call. The cache stays locked while `f`
    /// runs, so `f` must not come back to this document.
    fn cached_state_vector<R>(&self, f: impl FnOnce(&Vec<u8>) -> R) -> R {
        let revision = self.revision();
        let mut cache = self.sv_cache.lock().unwrap_or_else(|p| p.into_inner());
        let sv = match &mut *cache {
            Some((at, sv)) if *at == revision => sv,
            slot => &slot.insert((revision, self.doc.transact().state_vector().encode_v1())).1,
        };
        f(sv)
    }

    /// Encode the updates this document has that a peer at `their_state_vector`
    /// does not — the delta to bring that peer up to date (anti-entropy).
    ///
    /// **Only encode from the document that actually owns the data.** This is the
    /// correct anti-entropy primitive when a replica answers a peer's state vector
    /// with a delta from its *own* store. Do **not** use it to build a relay or
    /// fan-out — i.e. do not apply deltas into a separate "relay" document and then
    /// re-encode them with this method. Re-encoding state through a relay doc can
    /// silently **drop a record** for some actor-id orderings, because the relay
    /// recomputes the delta from its own (re-ordered) state instead of forwarding
    /// what it received. For relay / fan-out / append-to-a-sync-log scenarios, use
    /// [`take_local_updates`](Self::take_local_updates) and forward each raw update
    /// **verbatim** — those apply idempotently and never re-encode.
    pub fn encode_update_since(&self, their_state_vector: &[u8]) -> CrdtResult<Vec<u8>> {
        let revision = self.revision();
        let mut memo = self.delta_memo.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((at, sv, update)) = &*memo {
            if *at == revision && sv.as_slice() == their_state_vector {
                return Ok(update.clone());
            }
        }
        let sv = StateVector::decode_v1(their_state_vector)
            .map_err(|e| CrdtError::DecodeStateVector(e.to_string()))?;
        let update = self.doc.transact().encode_state_as_update_v1(&sv);
        if update.len() <= DELTA_MEMO_MAX {
            match &mut *memo {
                Some((at, sv, kept)) => {
                    *at = revision;
                    sv.clear();
                    sv.extend_from_slice(their_state_vector);
                    kept.clone_from(&update);
                }
                slot => *slot = Some((revision, their_state_vector.to_vec(), update.clone())),
            }
        }
        Ok(update)
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
        let mut txn = self.doc.transact_mut();
        let result = txn
            .apply_update(update)
            .map_err(|e| CrdtError::ApplyUpdate(e.to_string()));
        // A remote integration: accounted for, never re-buffered as local.
        self.finish(&mut txn, false);
        result
    }

    /// A causal read's question in one transaction: `Ok(state_vector)` if this
    /// document has caught up to `peer_state_vector` (the same bytes
    /// [`state_vector`](Self::state_vector) returns), else `Err(lag)` with
    /// [`ops_behind`](Self::ops_behind)'s count. Asking the two separately
    /// opened two transactions and built this document's state vector twice.
    pub fn caught_up_state_vector(
        &self,
        peer_state_vector: &[u8],
    ) -> CrdtResult<Result<Vec<u8>, usize>> {
        let peer = StateVector::decode_v1(peer_state_vector)
            .map_err(|e| CrdtError::DecodeStateVector(e.to_string()))?;
        let txn = self.doc.transact();
        let mine = txn.state_vector();
        let missing = missing_ops(&peer, &mine);
        Ok(if missing == 0 { Ok(mine.encode_v1()) } else { Err(missing) })
    }

    /// **Convergence lag**: how many operations a peer (described by its
    /// `peer_state_vector`) has that this replica has not yet seen. Zero means
    /// this replica is caught up to the peer's announced frontier. The replica
    /// layer uses this to report honest read freshness (`Live` vs `Stale{lag}`).
    pub fn ops_behind(&self, peer_state_vector: &[u8]) -> CrdtResult<usize> {
        let peer = StateVector::decode_v1(peer_state_vector)
            .map_err(|e| CrdtError::DecodeStateVector(e.to_string()))?;
        let txn = self.doc.transact();
        Ok(missing_ops(&peer, &txn.state_vector()))
    }
}

/// Several mutations inside one transaction; see [`CrdtDoc::batch`]. The
/// transaction commits when the batch is dropped, at the end of the closure.
pub struct Batch<'d> {
    doc: &'d CrdtDoc,
    txn: TransactionMut<'d>,
}

impl Batch<'_> {
    /// [`CrdtDoc::set_register`] inside the batch.
    pub fn set_register<T: Serialize>(&mut self, field: &str, value: &T) -> CrdtResult<()> {
        self.doc.set_register_in(&mut self.txn, field, value)
    }

    /// [`CrdtDoc::increment`] inside the batch.
    pub fn increment(&mut self, field: &str, delta: i64) {
        self.doc.increment_in(&mut self.txn, field, delta);
    }

    /// [`CrdtDoc::text_push`] inside the batch.
    pub fn text_push(&mut self, field: &str, content: &str) {
        CrdtDoc::text_push_in(&mut self.txn, field, content);
    }

    /// [`CrdtDoc::set_add`] inside the batch.
    pub fn set_add(&mut self, field: &str, element: &str) {
        CrdtDoc::set_add_in(&mut self.txn, field, element);
    }
}

/// How many operations `peer` has that `mine` lacks.
fn missing_ops(peer: &StateVector, mine: &StateVector) -> usize {
    let mut missing = 0usize;
    for (client, peer_clock) in peer.iter() {
        let my_clock = mine.get(client);
        if *peer_clock > my_clock {
            missing += (*peer_clock - my_clock) as usize;
        }
    }
    missing
}

#[cfg(test)]
mod stack_str_tests {
    #[test]
    fn stack_decimal_matches_display() {
        for v in [
            0i64, 1, -1, 9, 10, -10, 99, 100, 12345, -12345, i64::MAX, i64::MIN,
            i64::MIN + 1, 1_000_000_007, -999_999_999_999,
        ] {
            let mut s = super::StackStr::<24>::new();
            s.push_i64(v).unwrap();
            assert_eq!(s.as_str(), v.to_string(), "{v}");
        }
        let mut key = super::StackStr::<96>::new();
        key.push_counter_key("visits", 18446744073709551615).unwrap();
        assert_eq!(key.as_str(), super::CrdtDoc::counter_key("visits", u64::MAX));
        // Too long for the stack: an error, not a truncated key.
        let mut small = super::StackStr::<8>::new();
        assert!(small.push_counter_key("visits", 7).is_err());
    }
}
