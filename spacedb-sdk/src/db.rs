//! The [`Database`] — the offline-first local replica a developer holds.
//!
//! It owns the local CRDT documents (one per collection), a strong-tier quorum, an
//! mID key directory + revocation set, and a rate card. Every mutating op runs the
//! same spine: **authorize** (mID), **charge** (budget), **route by tier**, and
//! **return the honest [`Outcome`]** of what was actually achieved. Reads are local
//! and always available; sync is explicit CRDT merge, so the whole thing works with
//! no network at all.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use spacedb_access::{
    authorize, AccessRequest, Decision, Identity, MemKeyDirectory, Ops, RevocationSet, Scope,
    SignedCapability,
};
use spacedb_consistency::{Outcome, QuorumGroup, StrongResult, Tier};
use spacedb_crdt::{CrdtDoc, CrdtError, Watcher};
use spacedb_meter::{MeterError, RateCard, Usage};

use crate::error::{SdkError, SdkResult};
use crate::schema::{CrdtType, FieldSpec, Schema};
use crate::session::Session;

/// Fuel charged per mutating op (a flat, deterministic write cost for the demo;
/// a real deployment would meter actual work).
const WRITE_FUEL: u64 = 1_000;

/// An offline-first SpaceDB replica.
pub struct Database {
    node: Identity,
    actor_id: u64,
    directory: MemKeyDirectory,
    revocations: RevocationSet,
    rate_card: RateCard,
    clock: u64,
    schemas: HashMap<String, Schema>,
    docs: HashMap<String, CrdtDoc>,
    quorum: QuorumGroup,
}

impl Database {
    /// Open a local replica for `node`. In-memory and offline by construction;
    /// durability via `spacedb-store` and a real placed quorum are wiring the host
    /// supplies.
    pub fn open(node: Identity) -> Self {
        let actor_id = actor_id_for(&node.did().0);
        Self {
            node,
            actor_id,
            directory: MemKeyDirectory::new(),
            revocations: RevocationSet::new(),
            rate_card: default_rate_card(),
            clock: 0,
            schemas: HashMap::new(),
            docs: HashMap::new(),
            // a self-hosted 3-member strong-tier quorum (placement is the M4 seam)
            quorum: QuorumGroup::new(["q0", "q1", "q2"]),
        }
    }

    /// This replica's mID.
    pub fn node(&self) -> &Identity {
        &self.node
    }

    /// Publish an identity's key so capabilities it issues can be verified.
    pub fn register_identity(&self, identity: &Identity) -> SdkResult<()> {
        self.directory
            .publish(identity)
            .map_err(|e| SdkError::Auth(e.to_string()))
    }

    /// Register a collection's schema.
    pub fn define(&mut self, schema: Schema) {
        self.schemas.insert(schema.collection().to_string(), schema);
    }

    /// Set the logical clock used for capability expiry checks.
    pub fn set_clock(&mut self, now_unix: u64) {
        self.clock = now_unix;
    }

    /// The micro-`$MATA` cost of one mutating op under the current rate card.
    pub fn write_cost(&self) -> u64 {
        self.rate_card.price(&Usage::compute(WRITE_FUEL, 1))
    }

    /// Begin a session for the holder of `capability`.
    pub fn session(&self, capability: SignedCapability) -> Session {
        Session::from_capability(capability)
    }

    /// Revoke a capability by id (mID kill-switch).
    pub fn revoke(&mut self, capability_id: [u8; 16]) {
        self.revocations.revoke(capability_id);
    }

    // ── convergent / causal writes ──────────────────────────────────────────

    /// Set a `Register` field. Returns the honest [`Outcome`] (`Local` for
    /// convergent/causal — durable here, converging outward).
    pub fn put_register(
        &mut self,
        session: &mut Session,
        collection: &str,
        field: &str,
        value: &str,
    ) -> SdkResult<Outcome> {
        let spec = self.require_field(collection, field, CrdtType::Register)?;
        let tier = self.begin_write(session, collection, spec)?;
        let doc = self.doc_mut(collection);
        doc.set_register(field, &value.to_string()).map_err(crdt_err)?;
        Ok(local_outcome(tier, session, doc))
    }

    /// Add `delta` to a `Counter` field.
    pub fn increment(
        &mut self,
        session: &mut Session,
        collection: &str,
        field: &str,
        delta: i64,
    ) -> SdkResult<Outcome> {
        let spec = self.require_field(collection, field, CrdtType::Counter)?;
        let tier = self.begin_write(session, collection, spec)?;
        let doc = self.doc_mut(collection);
        doc.increment(field, delta);
        Ok(local_outcome(tier, session, doc))
    }

    /// Append to a `Text` field.
    pub fn append_text(
        &mut self,
        session: &mut Session,
        collection: &str,
        field: &str,
        text: &str,
    ) -> SdkResult<Outcome> {
        let spec = self.require_field(collection, field, CrdtType::Text)?;
        let tier = self.begin_write(session, collection, spec)?;
        let doc = self.doc_mut(collection);
        doc.text_push(field, text);
        Ok(local_outcome(tier, session, doc))
    }

    /// Add an element to a `Set` field.
    pub fn add_to_set(
        &mut self,
        session: &mut Session,
        collection: &str,
        field: &str,
        element: &str,
    ) -> SdkResult<Outcome> {
        let spec = self.require_field(collection, field, CrdtType::Set)?;
        let tier = self.begin_write(session, collection, spec)?;
        let doc = self.doc_mut(collection);
        doc.set_add(field, element);
        Ok(local_outcome(tier, session, doc))
    }

    // ── strong writes ───────────────────────────────────────────────────────

    /// Claim a globally-unique `value` for a strong-tier field (e.g. a username).
    /// Returns the quorum's honest [`StrongResult`]: `Committed`, `Rejected`
    /// (already taken), or `Unavailable` (no quorum — fails safe, commits nothing).
    pub fn claim_unique(
        &mut self,
        session: &mut Session,
        collection: &str,
        field: &str,
        value: &str,
    ) -> SdkResult<StrongResult> {
        let spec = self.require_field(collection, field, CrdtType::Register)?;
        if spec.tier != Tier::Strong {
            return Err(SdkError::WrongType {
                field: field.to_string(),
                expected: CrdtType::Register,
                found: spec.crdt,
            });
        }
        self.authorize_op(session, collection, Ops::WRITE)?;
        self.charge(session)?;
        let key = format!("{collection}/{field}/{value}");
        let owner = session.actor.0.as_bytes().to_vec();
        Ok(self.quorum.claim_unique(&key, &owner))
    }

    /// Who owns a claimed unique `value`, if anyone (a quorum read).
    pub fn unique_owner(
        &self,
        collection: &str,
        field: &str,
        value: &str,
    ) -> Option<String> {
        let key = format!("{collection}/{field}/{value}");
        match self.quorum.read(&key) {
            Ok((Some(bytes), _)) => Some(String::from_utf8_lossy(&bytes).into_owned()),
            _ => None,
        }
    }

    /// Partition a strong-tier quorum member (testing / chaos).
    pub fn quorum_partition(&mut self, member: &str) -> bool {
        self.quorum.partition(member)
    }

    /// Heal a strong-tier quorum member.
    pub fn quorum_heal(&mut self, member: &str) -> bool {
        self.quorum.heal(member)
    }

    // ── reads ───────────────────────────────────────────────────────────────

    /// Read a `Register`, with the honest consistency [`Outcome`] of the read.
    /// Requires read authorization.
    pub fn read_register(
        &self,
        session: &mut Session,
        collection: &str,
        field: &str,
    ) -> SdkResult<(Option<String>, Outcome)> {
        let spec = self.require_field(collection, field, CrdtType::Register)?;
        self.authorize_op(session, collection, Ops::READ)?;
        let doc = self.docs.get(collection);
        let value = match doc {
            Some(d) => d.get_register::<String>(field).map_err(crdt_err)?,
            None => None,
        };
        let outcome = match (spec.tier, doc) {
            (Tier::Causal, Some(d)) => session.causal.read(d),
            (Tier::Causal, None) => Outcome::Committed(Tier::Causal),
            (tier, _) => Outcome::Committed(tier),
        };
        Ok((value, outcome))
    }

    /// Local (always-available) reads — the convergent view on this replica.
    pub fn counter(&self, collection: &str, field: &str) -> i64 {
        self.docs.get(collection).map_or(0, |d| d.counter(field))
    }

    pub fn text(&self, collection: &str, field: &str) -> String {
        self.docs.get(collection).map_or_else(String::new, |d| d.text(field))
    }

    pub fn set_members(&self, collection: &str, field: &str) -> Vec<String> {
        self.docs
            .get(collection)
            .map_or_else(Vec::new, |d| d.set_members(field))
    }

    // ── reactive + sync ─────────────────────────────────────────────────────

    /// A reactive watcher for a collection: `drain_changed()` returns true after
    /// any local or merged change.
    pub fn watch(&mut self, collection: &str) -> Watcher {
        self.doc_mut(collection).watch()
    }

    /// Export this replica's state for a collection (offline sync).
    pub fn export(&self, collection: &str) -> Vec<u8> {
        self.docs
            .get(collection)
            .map_or_else(Vec::new, |d| d.encode_full())
    }

    /// Merge another replica's exported state into this one (convergent).
    pub fn import(&mut self, collection: &str, update: &[u8]) -> SdkResult<()> {
        self.doc_mut(collection).apply_update(update).map_err(crdt_err)
    }

    // ── internals ───────────────────────────────────────────────────────────

    fn require_field(
        &self,
        collection: &str,
        field: &str,
        expected: CrdtType,
    ) -> SdkResult<FieldSpec> {
        let schema = self
            .schemas
            .get(collection)
            .ok_or_else(|| SdkError::UnknownCollection(collection.to_string()))?;
        let spec = schema.spec(field).ok_or_else(|| SdkError::UnknownField {
            collection: collection.to_string(),
            field: field.to_string(),
        })?;
        if spec.crdt != expected {
            return Err(SdkError::WrongType {
                field: field.to_string(),
                expected,
                found: spec.crdt,
            });
        }
        Ok(spec)
    }

    /// The shared write prologue: reject strong puts, authorize, then charge.
    /// Returns the field's tier for outcome routing.
    fn begin_write(
        &self,
        session: &mut Session,
        collection: &str,
        spec: FieldSpec,
    ) -> SdkResult<Tier> {
        if spec.tier == Tier::Strong {
            return Err(SdkError::StrongFieldNeedsClaim(collection.to_string()));
        }
        self.authorize_op(session, collection, Ops::WRITE)?;
        self.charge(session)?;
        Ok(spec.tier)
    }

    fn authorize_op(&self, session: &Session, collection: &str, op: Ops) -> SdkResult<()> {
        let scope = Scope::Collection(collection.to_string());
        let request = AccessRequest {
            bearer: &session.actor,
            scope: &scope,
            op,
        };
        match authorize(
            &session.capability,
            &request,
            &self.directory,
            self.clock,
            &self.revocations,
        ) {
            Ok(Decision::Allow) => Ok(()),
            Ok(Decision::Deny(reason)) => Err(SdkError::Denied(reason)),
            Err(e) => Err(SdkError::Auth(e.to_string())),
        }
    }

    fn charge(&self, session: &mut Session) -> SdkResult<()> {
        let cost = self.write_cost();
        session.budget.charge(cost).map_err(|e| match e {
            MeterError::OverBudget { cost, remaining } => SdkError::OverBudget { cost, remaining },
            other => SdkError::Auth(other.to_string()),
        })
    }

    fn doc_mut(&mut self, collection: &str) -> &CrdtDoc {
        let actor = self.actor_id;
        self.docs
            .entry(collection.to_string())
            .or_insert_with(|| CrdtDoc::new(actor))
    }
}

/// Convergent/causal writes are locally durable (`Local`); a causal field also
/// advances the session's causal token so it reads its own write.
fn local_outcome(tier: Tier, session: &mut Session, doc: &CrdtDoc) -> Outcome {
    match tier {
        Tier::Causal => session.causal.record_write(doc),
        _ => Outcome::Local,
    }
}

fn actor_id_for(did: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    did.hash(&mut hasher);
    hasher.finish() | 1 // never 0
}

fn default_rate_card() -> RateCard {
    RateCard {
        storage_per_gib_month: 5_000_000,
        compute_per_megafuel: 1_000_000,
        compute_per_invocation: 1_000,
        transit_per_gib: 1_000_000,
    }
}

fn crdt_err(e: CrdtError) -> SdkError {
    SdkError::Crdt(e.to_string())
}
