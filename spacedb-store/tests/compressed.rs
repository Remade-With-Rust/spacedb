//! W1.2 integration gates: prefixed (optionally compressed) collections, and
//! the legacy-compatibility story — a pre-v2 store keeps working, byte layout
//! unchanged, under the new code.

use std::sync::Arc;

use spacedb_store::collection::{COLLECTION_FORMATS_TABLE, DEK_WRAPPINGS_TABLE};
use spacedb_store::{
    open_meta, KeyEncode, wrap_fresh_dek, write_store_version, Collection, Compression, Durability,
    KeyProvider, KvEngine, MemEngine, MetaStatus, Readable, StaticKeyProvider, StoreError, Table,
    WrappedDek, WriteTx, STORE_FORMAT_VERSION,
};

const VAULT_KEY: [u8; 32] = [0x42; 32];

fn provider() -> Arc<dyn KeyProvider> {
    Arc::new(StaticKeyProvider::new(VAULT_KEY))
}

/// A row payload with realistic structure (JSON-ish, repetitive) — compresses.
fn compressible_value(n: usize) -> String {
    format!("{{\"kind\":\"password\",\"site\":\"https://example.com/login\",\"user\":\"ada@example.com\",\"notes\":\"{}\"}}",
        "the quick brown fox jumps over the lazy dog ".repeat(n))
}

/// AES-256-GCM overhead per sealed row: 12-byte nonce + 16-byte tag.
const SEAL_OVERHEAD: usize = 12 + 16;

/// What old (pre-v2) code did to create a collection: provision the DEK
/// wrapping and nothing else — no `_collection_formats` entry.
fn create_legacy_collection(engine: &MemEngine, name: &str) {
    let (wrapped, _dek) = wrap_fresh_dek(&VAULT_KEY, name).unwrap();
    let table: Table<String, WrappedDek> = Table::new(DEK_WRAPPINGS_TABLE);
    let mut w = engine
        .begin_write(Durability::Immediate)
        .unwrap();
    table.put(&mut w, &name.to_string(), &wrapped).unwrap();
    w.commit().unwrap();
}


#[test]
fn new_collections_compress_on_the_engine_side() {
    let engine = MemEngine::new();
    let col: Collection<String, String> =
        Collection::open_or_create(&engine, provider(), "vault", 1).unwrap();

    let value = compressible_value(50); // ~2.3 KiB, highly repetitive
    let mut w = engine
        .begin_write(Durability::Immediate)
        .unwrap();
    col.put(&mut w, &"row1".to_string(), &value).unwrap();
    w.commit().unwrap();

    // The engine-side sealed bytes must be materially smaller than the
    // plaintext — proof compression engaged through the full stack, not just
    // in a unit test.
    let r = engine.begin_read().unwrap();
    let sealed = r.get_raw("vault", &KeyEncode::encode(&"row1".to_string())).unwrap().unwrap();
    assert!(
        sealed.len() < value.len() / 2,
        "sealed {} bytes not < half of plaintext {} — compression did not engage",
        sealed.len(),
        value.len()
    );

    // And it round-trips.
    let got = col.get(&r, &"row1".to_string()).unwrap().unwrap();
    assert_eq!(got, value);
}

#[test]
fn incompressible_rows_cost_exactly_one_byte() {
    let engine = MemEngine::new();
    let col: Collection<String, Vec<u8>> =
        Collection::open_or_create(&engine, provider(), "blobs", 1).unwrap();

    // Pseudo-random bytes: incompressible, so the row stores raw + 1 format byte.
    let mut state = 0xDEADBEEFCAFEu64;
    let value: Vec<u8> = (0..2048u32)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 56) as u8
        })
        .collect();

    let mut w = engine
        .begin_write(Durability::Immediate)
        .unwrap();
    col.put(&mut w, &"b".to_string(), &value).unwrap();
    w.commit().unwrap();

    let r = engine.begin_read().unwrap();
    let sealed = r.get_raw("blobs", &KeyEncode::encode(&"b".to_string())).unwrap().unwrap();
    // postcard length-prefixes the Vec (varint, 2 bytes at this size).
    let encoded_len = value.len() + 2;
    assert_eq!(
        sealed.len(),
        encoded_len + 1 + SEAL_OVERHEAD,
        "incompressible row should be exactly encoded + 1 format byte + AEAD overhead"
    );
    assert_eq!(col.get(&r, &"b".to_string()).unwrap().unwrap(), value);
}

#[test]
fn legacy_collections_stay_legacy_and_unprefixed() {
    let engine = MemEngine::new();
    create_legacy_collection(&engine, "oldvault");

    // New code opens the legacy collection: no format entry -> legacy layout.
    let col: Collection<String, String> =
        Collection::open(&engine, provider(), "oldvault", 1).unwrap();

    let value = "short secret".to_string();
    let mut w = engine
        .begin_write(Durability::Immediate)
        .unwrap();
    col.put(&mut w, &"k".to_string(), &value).unwrap();
    w.commit().unwrap();

    // Byte-layout proof: sealed = postcard(value) + AEAD overhead, NO +1. A
    // pre-v2 binary reading this store sees exactly the bytes it always wrote.
    let r = engine.begin_read().unwrap();
    let sealed = r.get_raw("oldvault", &KeyEncode::encode(&"k".to_string())).unwrap().unwrap();
    let encoded_len = value.len() + 1; // postcard varint length prefix
    assert_eq!(sealed.len(), encoded_len + SEAL_OVERHEAD, "legacy row grew a prefix");
    assert_eq!(col.get(&r, &"k".to_string()).unwrap().unwrap(), value);
    // Release the read tx: MemEngine blocks a writer while a reader is open,
    // and open_or_create below begins a write transaction.
    drop(r);

    // open_or_create on an existing legacy collection must NOT convert it.
    let again: Collection<String, String> =
        Collection::open_or_create(&engine, provider(), "oldvault", 1).unwrap();
    let r = engine.begin_read().unwrap();
    assert_eq!(again.get(&r, &"k".to_string()).unwrap().unwrap(), value);
    let fmt_table: Table<String, u8> = Table::new(COLLECTION_FORMATS_TABLE);
    assert!(
        fmt_table.get(&r, &"oldvault".to_string()).unwrap().is_none(),
        "open_or_create silently converted a legacy collection"
    );
}

#[test]
fn off_and_on_handles_interoperate_on_one_collection() {
    let engine = MemEngine::new();
    let on: Collection<String, String> =
        Collection::open_or_create(&engine, provider(), "mixed", 1).unwrap();
    let off: Collection<String, String> = Collection::open_with(
        &engine,
        provider(),
        "mixed",
        1,
        Compression::Off,
    )
    .unwrap();

    let value = compressible_value(20);
    let mut w = engine
        .begin_write(Durability::Immediate)
        .unwrap();
    on.put(&mut w, &"compressed".to_string(), &value).unwrap();
    off.put(&mut w, &"raw".to_string(), &value).unwrap();
    w.commit().unwrap();

    // Reads honor the per-row byte regardless of which handle reads.
    let r = engine.begin_read().unwrap();
    assert_eq!(off.get(&r, &"compressed".to_string()).unwrap().unwrap(), value);
    assert_eq!(on.get(&r, &"raw".to_string()).unwrap().unwrap(), value);

    // And the two rows really took different formats on the engine side.
    let sealed_c = r.get_raw("mixed", &KeyEncode::encode(&"compressed".to_string())).unwrap().unwrap();
    let sealed_r = r.get_raw("mixed", &KeyEncode::encode(&"raw".to_string())).unwrap().unwrap();
    assert!(sealed_c.len() < sealed_r.len() / 2);
}

#[test]
fn v1_store_migrates_by_stamp_and_v3_is_refused() {
    let engine = MemEngine::new();

    // A store stamped v1 (what every pre-v2 deployment recorded).
    write_store_version(&engine, 1).unwrap();
    match open_meta(&engine).unwrap() {
        MetaStatus::Migrated { from } => assert_eq!(from, 1),
        other => panic!("expected Migrated from 1, got {other:?}"),
    }
    // Idempotent: a second open is Current.
    assert!(matches!(open_meta(&engine).unwrap(), MetaStatus::Current));

    // A store from the future is refused, never misread.
    let engine = MemEngine::new();
    write_store_version(&engine, STORE_FORMAT_VERSION + 1).unwrap();
    match open_meta(&engine) {
        Err(StoreError::SchemaTooNew { found, supported }) => {
            assert_eq!(found, STORE_FORMAT_VERSION + 1);
            assert_eq!(supported, STORE_FORMAT_VERSION);
        }
        other => panic!("expected SchemaTooNew, got {other:?}"),
    }
}
