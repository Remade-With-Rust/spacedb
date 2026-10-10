//! Deterministic instruction-count driver for the CRDT document and its
//! persistence (callgrind Ir). yrs draws client ids from fastrand's thread-local
//! generator, which seeds itself from the clock — so the driver seeds it first,
//! or state-vector lengths and hash-map orders drift between runs.

use std::sync::Arc;

use spacedb_crdt::{CrdtDoc, CrdtStore};
use spacedb_store::{KeyProvider, MemEngine, StaticKeyProvider};

fn main() {
    fastrand::seed(7);
    let mut check: i64 = 0;

    // One document, every CRDT type, many small ops.
    let doc = CrdtDoc::new(1);
    for i in 0..1500 {
        doc.increment(["visits", "likes", "views"][i % 3], (i % 5) as i64 - 1);
    }
    for _ in 0..300 {
        check += doc.counter("visits") + doc.counter("likes");
    }
    for i in 0..200 {
        doc.set_add("tags", &format!("tag-{}", i % 50));
    }
    let mut hits = 0;
    for i in 0..300 {
        hits += doc.set_contains("tags", &format!("tag-{}", i % 80)) as i64;
    }
    for i in 0..300 {
        doc.set_register(&format!("field{}", i % 20), &format!("value number {i}"))
            .unwrap();
    }
    for i in 0..300 {
        let v: Option<String> = doc.get_register(&format!("field{}", i % 25)).unwrap();
        check += v.map(|s| s.len() as i64).unwrap_or(0);
    }
    for i in 0..200 {
        doc.text_push("bio", if i % 2 == 0 { "hello " } else { "world " });
    }
    check += doc.text_len("bio") as i64 + doc.register_keys().len() as i64;

    // Many actors merging into one replica (anti-entropy shape).
    let hub = CrdtDoc::new(1000);
    for actor in 0..24u64 {
        let d = CrdtDoc::new(2000 + actor);
        for _ in 0..40 {
            d.increment("writes", 1);
        }
        let delta = d.encode_update_since(&hub.state_vector()).unwrap();
        hub.apply_update(&delta).unwrap();
        check += hub.counter("writes");
    }
    let behind = doc.ops_behind(&hub.state_vector()).unwrap() as i64;

    // Persistence: save / load / apply_remote / contains through the sealed store.
    let engine = MemEngine::new();
    let provider: Arc<dyn KeyProvider> = Arc::new(StaticKeyProvider::new([7; 32]));
    let store = CrdtStore::open(&engine, provider).unwrap();
    store.save(&engine, "profile", &doc).unwrap();
    store.save(&engine, "hub", &hub).unwrap();
    for i in 0..10 {
        let loaded = store.load(&engine, "profile", 50 + i).unwrap();
        check += loaded.counter("likes");
    }
    let remote = CrdtDoc::new(9000);
    for i in 0..30 {
        remote.increment("writes", 1);
        let up = remote.encode_full();
        let merged = store.apply_remote(&engine, "hub", 1000, &up).unwrap();
        check += merged.counter("writes") * (i + 1);
    }
    let mut present = 0;
    for i in 0..200 {
        present += store
            .contains(&engine, if i % 2 == 0 { "hub" } else { "absent" })
            .unwrap() as i64;
    }

    println!(
        "hits {hits} behind {behind} present {present} full {} check {check}",
        doc.encode_full().len()
    );
}
