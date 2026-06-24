//! M2-S3: the richer field types (Y.Text, OR-Set), their tombstone/observed-
//! remove semantics, and content GC.

use std::sync::Arc;

use spacedb_crdt::{CrdtDoc, CrdtStore};
use spacedb_store::{KeyProvider, MemEngine, StaticKeyProvider};

fn warm() -> Arc<dyn KeyProvider> {
    Arc::new(StaticKeyProvider::new([5u8; 32]))
}

// ─── Y.Text ──────────────────────────────────────────────────────────────────

#[test]
fn text_push_insert_remove_and_read() {
    let d = CrdtDoc::new(1);
    d.text_push("body", "Hello");
    d.text_push("body", " World");
    assert_eq!(d.text("body"), "Hello World");
    assert_eq!(d.text_len("body"), 11);

    d.text_insert("body", 5, ",");
    assert_eq!(d.text("body"), "Hello, World");

    d.text_remove("body", 0, 5); // drop "Hello"
    assert_eq!(d.text("body"), ", World");
}

#[test]
fn text_missing_field_is_empty() {
    let d = CrdtDoc::new(1);
    assert_eq!(d.text("nope"), "");
    assert_eq!(d.text_len("nope"), 0);
}

#[test]
fn concurrent_text_edits_converge() {
    let a = CrdtDoc::new(10);
    let b = CrdtDoc::new(20);
    a.text_push("doc", "Hello ");
    b.text_push("doc", "World");
    // exchange
    b.apply_update(&a.encode_full()).unwrap();
    a.apply_update(&b.encode_full()).unwrap();
    assert_eq!(a.text("doc"), b.text("doc"), "text converges to one interleaving");
    assert!(a.text("doc").contains("Hello"));
    assert!(a.text("doc").contains("World"));
}

// ─── OR-Set ──────────────────────────────────────────────────────────────────

#[test]
fn set_add_contains_members_and_remove() {
    let d = CrdtDoc::new(1);
    d.set_add("tags", "a");
    d.set_add("tags", "b");
    d.set_add("tags", "a"); // duplicate add
    assert!(d.set_contains("tags", "a"));
    assert!(d.set_contains("tags", "b"));
    assert_eq!(d.set_members("tags"), vec!["a".to_string(), "b".to_string()]);

    // observed-remove drops every occurrence this replica saw
    d.set_remove("tags", "a");
    assert!(!d.set_contains("tags", "a"));
    assert_eq!(d.set_members("tags"), vec!["b".to_string()]);
}

#[test]
fn or_set_is_add_wins_on_concurrent_add_remove() {
    let a = CrdtDoc::new(10);
    let b = CrdtDoc::new(20);

    // both observe an initial add of "x"
    a.set_add("tags", "x");
    b.apply_update(&a.encode_full()).unwrap();
    a.apply_update(&b.encode_full()).unwrap();
    assert!(a.set_contains("tags", "x") && b.set_contains("tags", "x"));

    // concurrently: a removes the "x" it observed; b re-adds a fresh "x"
    let a_sv = a.state_vector();
    let b_sv = b.state_vector();
    a.set_remove("tags", "x");
    b.set_add("tags", "x");

    // exchange the deltas
    let a_delta = a.encode_update_since(&b_sv).unwrap();
    let b_delta = b.encode_update_since(&a_sv).unwrap();
    a.apply_update(&b_delta).unwrap();
    b.apply_update(&a_delta).unwrap();

    // the concurrent re-add the remover never saw survives — add wins
    assert!(a.set_contains("tags", "x"), "add-wins: concurrent re-add survives the remove");
    assert!(b.set_contains("tags", "x"));
    assert_eq!(a.set_members("tags"), b.set_members("tags"));
}

#[test]
fn set_removes_survive_reload() {
    let e = MemEngine::new();
    let store = CrdtStore::open(&e, warm()).unwrap();

    let d = CrdtDoc::new(1);
    d.set_add("tags", "keep");
    d.set_add("tags", "drop");
    d.set_remove("tags", "drop");
    store.save(&e, "s", &d).unwrap();

    let reloaded = store.load(&e, "s", 1).unwrap();
    assert!(reloaded.set_contains("tags", "keep"));
    assert!(!reloaded.set_contains("tags", "drop"), "tombstone must persist across reload");
    assert_eq!(reloaded.set_members("tags"), vec!["keep".to_string()]);
}

// ─── compaction / GC ─────────────────────────────────────────────────────────

#[test]
fn gc_frees_deleted_text_content() {
    let d = CrdtDoc::new(1);
    let big = "x".repeat(20_000);
    d.text_push("body", &big);
    let size_with_content = d.estimated_state_size();
    assert!(size_with_content > 20_000, "the text content is part of the state");

    d.text_remove("body", 0, 20_000);
    let size_after_delete = d.estimated_state_size();
    assert_eq!(d.text("body"), "");
    assert!(
        size_after_delete < size_with_content,
        "yrs GC must free deleted text content ({size_after_delete} !< {size_with_content})"
    );
}
