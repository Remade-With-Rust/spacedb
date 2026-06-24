//! [`CrdtStore`] — persisting convergent documents into the encrypted store.
//!
//! This is where Layer 1 (CRDT) meets Layer 0 (storage). Each document's CRDT
//! state is stored as the value of a row in an **encrypted** `spacedb-store`
//! [`Collection`] keyed by document id — so the on-disk bytes are AEAD ciphertext
//! the engine cannot read (the zero-knowledge posture holds for CRDT data too).
//!
//! The write path is the mission's loop made concrete: a local mutation applies
//! to the in-memory [`CrdtDoc`], and [`CrdtStore::save`] persists the new state in
//! **one M1 write transaction**. The incremental update to ship to peers comes
//! from the doc's own [`CrdtDoc::encode_update_since`] — the anti-entropy
//! primitive — and [`CrdtStore::apply_remote`] is the receive side
//! (load → merge → re-persist).
//!
//! **S2 stores a full state snapshot per save.** That is simple and correct; the
//! op-log + convergent compaction that avoids re-encoding the whole document on
//! every write is M2-S3.

use std::sync::Arc;

use spacedb_store::{Collection, Durability, KeyProvider, KvEngine, WriteTx};

use crate::{CrdtDoc, CrdtResult};

/// The encrypted collection that holds `doc_id → CRDT-state` rows.
pub const CRDT_DOCS_COLLECTION: &str = "crdt_docs";

/// The schema version bound into each persisted row's AEAD (see `spacedb-store`).
const SCHEMA_VERSION: u32 = 1;

/// Persists [`CrdtDoc`]s into an encrypted `spacedb-store` collection. Engine-
/// agnostic like the underlying [`Collection`]: the methods take the engine and
/// open their own transactions.
pub struct CrdtStore {
    docs: Collection<String, Vec<u8>>,
}

impl CrdtStore {
    /// Open (or first-time provision) the CRDT document collection on `engine`.
    pub fn open<E: KvEngine>(engine: &E, key_provider: Arc<dyn KeyProvider>) -> CrdtResult<Self> {
        let docs =
            Collection::open_or_create(engine, key_provider, CRDT_DOCS_COLLECTION, SCHEMA_VERSION)?;
        Ok(Self { docs })
    }

    /// Persist `doc`'s full CRDT state under `doc_id`, encrypted, in a single
    /// write transaction.
    pub fn save<E: KvEngine>(&self, engine: &E, doc_id: &str, doc: &CrdtDoc) -> CrdtResult<()> {
        let state = doc.encode_full();
        let mut w = engine.begin_write(Durability::Immediate)?;
        self.docs.put(&mut w, &doc_id.to_string(), &state)?;
        w.commit()?;
        Ok(())
    }

    /// Load the document stored under `doc_id` for replica `actor_id`. Returns a
    /// fresh empty document if nothing has been persisted there yet (local-first:
    /// you can always start writing).
    pub fn load<E: KvEngine>(
        &self,
        engine: &E,
        doc_id: &str,
        actor_id: u64,
    ) -> CrdtResult<CrdtDoc> {
        let stored = {
            let r = engine.begin_read()?;
            self.docs.get(&r, &doc_id.to_string())?
        };
        let doc = CrdtDoc::new(actor_id);
        if let Some(state) = stored {
            doc.apply_update(&state)?;
        }
        Ok(doc)
    }

    /// Receive side of anti-entropy: merge a remote `update` into the persisted
    /// document and re-persist it (load → merge → save). Returns the merged doc.
    pub fn apply_remote<E: KvEngine>(
        &self,
        engine: &E,
        doc_id: &str,
        actor_id: u64,
        update: &[u8],
    ) -> CrdtResult<CrdtDoc> {
        let doc = self.load(engine, doc_id, actor_id)?;
        doc.apply_update(update)?;
        self.save(engine, doc_id, &doc)?;
        Ok(doc)
    }

    /// Whether a document has been persisted under `doc_id`.
    pub fn contains<E: KvEngine>(&self, engine: &E, doc_id: &str) -> CrdtResult<bool> {
        let r = engine.begin_read()?;
        Ok(self.docs.get(&r, &doc_id.to_string())?.is_some())
    }
}
