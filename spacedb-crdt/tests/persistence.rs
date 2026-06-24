//! M2-S2: persisting convergent documents into the encrypted store, and proving
//! persistence + anti-entropy compose end to end.

use std::sync::Arc;

use spacedb_crdt::{CrdtDoc, CrdtStore, CRDT_DOCS_COLLECTION};
use spacedb_store::{
    KeyEncode, KeyProvider, KvEngine, MemEngine, Readable, RedbEngine, StaticKeyProvider,
};

fn warm() -> Arc<dyn KeyProvider> {
    Arc::new(StaticKeyProvider::new([9u8; 32]))
}

// ─── single-engine scenarios ─────────────────────────────────────────────────

fn save_then_load_round_trips(e: &impl KvEngine) {
    let store = CrdtStore::open(e, warm()).unwrap();
    let doc = CrdtDoc::new(1);
    doc.set_register("title", &"hello".to_string()).unwrap();
    doc.increment("views", 7);
    store.save(e, "doc1", &doc).unwrap();

    let loaded = store.load(e, "doc1", 1).unwrap();
    assert_eq!(loaded.get_register::<String>("title").unwrap(), Some("hello".to_string()));
    assert_eq!(loaded.counter("views"), 7);
}

fn load_missing_returns_empty_doc(e: &impl KvEngine) {
    let store = CrdtStore::open(e, warm()).unwrap();
    assert!(!store.contains(e, "nope").unwrap());
    let doc = store.load(e, "nope", 1).unwrap();
    assert_eq!(doc.get_register::<String>("x").unwrap(), None);
    assert_eq!(doc.counter("c"), 0);
}

fn mutate_save_reload_loop(e: &impl KvEngine) {
    let store = CrdtStore::open(e, warm()).unwrap();

    let d1 = store.load(e, "d", 1).unwrap();
    d1.set_register("n", &1u64).unwrap();
    d1.increment("c", 2);
    store.save(e, "d", &d1).unwrap();

    let d2 = store.load(e, "d", 1).unwrap();
    d2.set_register("n", &2u64).unwrap();
    d2.increment("c", 3);
    store.save(e, "d", &d2).unwrap();

    let d3 = store.load(e, "d", 1).unwrap();
    assert_eq!(d3.get_register::<u64>("n").unwrap(), Some(2));
    assert_eq!(d3.counter("c"), 5);
}

fn crdt_state_encrypted_at_rest(e: &impl KvEngine) {
    const MARKER: &[u8] = b"SECRET-PAYLOAD";
    let store = CrdtStore::open(e, warm()).unwrap();
    let doc = CrdtDoc::new(1);
    doc.set_register("note", &"SECRET-PAYLOAD-and-more".to_string()).unwrap();
    store.save(e, "secret-doc", &doc).unwrap();

    // The raw bytes the engine holds for this document must be ciphertext.
    let r = e.begin_read().unwrap();
    let raw = r
        .get_raw(CRDT_DOCS_COLLECTION, &"secret-doc".to_string().encode())
        .unwrap()
        .expect("row present");
    assert!(
        !raw.windows(MARKER.len()).any(|w| w == MARKER),
        "CRDT document state must be encrypted at rest"
    );

    // ...and it decrypts back through the store.
    let loaded = store.load(e, "secret-doc", 1).unwrap();
    assert_eq!(
        loaded.get_register::<String>("note").unwrap(),
        Some("SECRET-PAYLOAD-and-more".to_string())
    );
}

// ─── two homes: persistence + anti-entropy converge ──────────────────────────

fn two_homes_persist_and_converge<E: KvEngine>(ea: &E, eb: &E) {
    let store_a = CrdtStore::open(ea, warm()).unwrap();
    let store_b = CrdtStore::open(eb, warm()).unwrap();

    // Concurrent local edits on two homes, each persisted under its own DEK.
    let a = CrdtDoc::new(10);
    a.set_register("status", &"from-a".to_string()).unwrap();
    a.increment("n", 3);
    store_a.save(ea, "shared", &a).unwrap();

    let b = CrdtDoc::new(20);
    b.set_register("status", &"from-b".to_string()).unwrap();
    b.increment("n", 5);
    store_b.save(eb, "shared", &b).unwrap();

    // Anti-entropy: exchange the deltas each peer is missing (state-vector based).
    let delta_b_to_a = b.encode_update_since(&a.state_vector()).unwrap();
    let merged_a = store_a.apply_remote(ea, "shared", 10, &delta_b_to_a).unwrap();
    let delta_a_to_b = a.encode_update_since(&b.state_vector()).unwrap();
    let merged_b = store_b.apply_remote(eb, "shared", 20, &delta_a_to_b).unwrap();

    // Both converge (counter sums; register picks the same LWW winner).
    assert_eq!(merged_a.counter("n"), 8);
    assert_eq!(merged_b.counter("n"), 8);
    assert_eq!(
        merged_a.get_register::<String>("status").unwrap(),
        merged_b.get_register::<String>("status").unwrap()
    );

    // The converged state was persisted on both homes (reload to confirm).
    let reloaded_a = store_a.load(ea, "shared", 10).unwrap();
    let reloaded_b = store_b.load(eb, "shared", 20).unwrap();
    assert_eq!(reloaded_a.counter("n"), 8);
    assert_eq!(reloaded_b.counter("n"), 8);
    assert_eq!(
        reloaded_a.get_register::<String>("status").unwrap(),
        reloaded_b.get_register::<String>("status").unwrap()
    );
}

#[test]
fn two_homes_converge_mem() {
    two_homes_persist_and_converge(&MemEngine::new(), &MemEngine::new());
}

#[test]
fn two_homes_converge_redb() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let ea = RedbEngine::open(da.path().join("a.redb")).unwrap();
    let eb = RedbEngine::open(db.path().join("b.redb")).unwrap();
    two_homes_persist_and_converge(&ea, &eb);
}

// ─── durability across a store reopen (redb only) ────────────────────────────

#[test]
fn crdt_state_persists_across_store_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("persist.redb");

    {
        let e = RedbEngine::open(&path).unwrap();
        let store = CrdtStore::open(&e, warm()).unwrap();
        let doc = CrdtDoc::new(1);
        doc.set_register("k", &"durable".to_string()).unwrap();
        doc.increment("c", 9);
        store.save(&e, "d", &doc).unwrap();
    }

    // Reopen a fresh engine + store on the same file, same vault key.
    let e2 = RedbEngine::open(&path).unwrap();
    let store2 = CrdtStore::open(&e2, warm()).unwrap();
    let loaded = store2.load(&e2, "d", 1).unwrap();
    assert_eq!(loaded.get_register::<String>("k").unwrap(), Some("durable".to_string()));
    assert_eq!(loaded.counter("c"), 9);
}

// ─── run single-engine scenarios against both engines ────────────────────────

macro_rules! engine_suite {
    ($modname:ident, $make:expr, [$($scenario:ident),* $(,)?]) => {
        mod $modname {
            use super::*;
            $(
                #[test]
                fn $scenario() {
                    let (_holder, engine) = $make;
                    super::$scenario(&engine);
                }
            )*
        }
    };
}

engine_suite!(
    mem,
    ((), MemEngine::new()),
    [
        save_then_load_round_trips,
        load_missing_returns_empty_doc,
        mutate_save_reload_loop,
        crdt_state_encrypted_at_rest,
    ]
);

engine_suite!(
    redb,
    {
        let dir = tempfile::tempdir().unwrap();
        let engine = RedbEngine::open(dir.path().join("store.redb")).unwrap();
        (dir, engine)
    },
    [
        save_then_load_round_trips,
        load_missing_returns_empty_doc,
        mutate_save_reload_loop,
        crdt_state_encrypted_at_rest,
    ]
);
