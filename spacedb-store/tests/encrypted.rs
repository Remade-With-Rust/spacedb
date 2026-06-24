//! The AEAD value boundary, end to end, over both engines.
//!
//! These are the *real* versions of the properties a fake test would only
//! pretend to check: the engine bytes are genuinely ciphertext (not a zero-keyed
//! placeholder), tampering at the engine level is caught, a wrong vault key
//! cannot read, and a cold vault blocks access.

use std::sync::Arc;

use spacedb_store::{
    Collection, Durability, KeyEncode, KeyProvider, KvEngine, MemEngine, Readable, RedbEngine,
    StaticKeyProvider, StoreError, WriteTx,
};

fn warm() -> Arc<dyn KeyProvider> {
    Arc::new(StaticKeyProvider::new([7u8; 32]))
}

// ─── scenarios ───────────────────────────────────────────────────────────────

fn round_trip(e: &impl KvEngine) {
    let c: Collection<u64, String> = Collection::open_or_create(e, warm(), "people", 1).unwrap();
    {
        let mut w = e.begin_write(Durability::Immediate).unwrap();
        c.put(&mut w, &1, &"alice".to_string()).unwrap();
        c.put(&mut w, &2, &"bob".to_string()).unwrap();
        w.commit().unwrap();
    }
    let r = e.begin_read().unwrap();
    assert_eq!(c.get(&r, &1).unwrap(), Some("alice".to_string()));
    assert_eq!(c.get(&r, &2).unwrap(), Some("bob".to_string()));
    assert_eq!(c.get(&r, &3).unwrap(), None);
}

fn engine_stores_ciphertext_not_plaintext(e: &impl KvEngine) {
    const MARKER: &[u8] = b"TOP-SECRET-VALUE";
    let c: Collection<u64, String> = Collection::open_or_create(e, warm(), "secrets", 1).unwrap();
    {
        let mut w = e.begin_write(Durability::Immediate).unwrap();
        c.put(&mut w, &42, &"TOP-SECRET-VALUE-and-then-some".to_string()).unwrap();
        w.commit().unwrap();
    }
    let r = e.begin_read().unwrap();
    // Read the RAW stored bytes the engine holds for this row.
    let raw = r.get_raw(c.name(), &42u64.encode()).unwrap().expect("row present");
    assert!(
        !raw.windows(MARKER.len()).any(|win| win == MARKER),
        "the engine must store ciphertext — the plaintext marker leaked"
    );
    // ...and it genuinely decrypts back through the collection.
    assert_eq!(
        c.get(&r, &42).unwrap(),
        Some("TOP-SECRET-VALUE-and-then-some".to_string())
    );
}

fn tamper_at_engine_level_is_rejected(e: &impl KvEngine) {
    let c: Collection<u64, String> = Collection::open_or_create(e, warm(), "t", 1).unwrap();
    {
        let mut w = e.begin_write(Durability::Immediate).unwrap();
        c.put(&mut w, &1, &"v".to_string()).unwrap();
        w.commit().unwrap();
    }
    // Corrupt the stored ciphertext directly (flip a tag byte).
    let key_bytes = 1u64.encode();
    {
        let mut w = e.begin_write(Durability::Immediate).unwrap();
        let mut raw = w.get_raw(c.name(), &key_bytes).unwrap().unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0x01;
        w.put_raw(c.name(), &key_bytes, &raw).unwrap();
        w.commit().unwrap();
    }
    let r = e.begin_read().unwrap();
    assert!(
        matches!(c.get(&r, &1).unwrap_err(), StoreError::Crypto(_)),
        "a tampered row must fail AEAD verification"
    );
}

fn cold_provider_blocks_access(e: &impl KvEngine) {
    // Provision + populate under a warm provider.
    let c: Collection<u64, String> = Collection::open_or_create(e, warm(), "c", 1).unwrap();
    {
        let mut w = e.begin_write(Durability::Immediate).unwrap();
        c.put(&mut w, &1, &"v".to_string()).unwrap();
        w.commit().unwrap();
    }
    // Re-open with a COLD provider — `open` only reads the wrapping, no key.
    let cold: Arc<dyn KeyProvider> = Arc::new(StaticKeyProvider::cold());
    let c_cold: Collection<u64, String> = Collection::open(e, cold, "c", 1).unwrap();

    let r = e.begin_read().unwrap();
    assert!(matches!(c_cold.get(&r, &1).unwrap_err(), StoreError::Cold));
    // An absent row still returns None without requiring an unlock.
    assert_eq!(c_cold.get(&r, &999).unwrap(), None);
    drop(r);

    let mut w = e.begin_write(Durability::Immediate).unwrap();
    assert!(matches!(
        c_cold.put(&mut w, &2, &"x".to_string()).unwrap_err(),
        StoreError::Cold
    ));
    drop(w);
}

fn wrong_vault_key_cannot_decrypt(e: &impl KvEngine) {
    let key_a: Arc<dyn KeyProvider> = Arc::new(StaticKeyProvider::new([1u8; 32]));
    let key_b: Arc<dyn KeyProvider> = Arc::new(StaticKeyProvider::new([2u8; 32]));

    let c: Collection<u64, String> = Collection::open_or_create(e, key_a, "k", 1).unwrap();
    {
        let mut w = e.begin_write(Durability::Immediate).unwrap();
        c.put(&mut w, &1, &"v".to_string()).unwrap();
        w.commit().unwrap();
    }
    // A different vault key can't unwrap the DEK, so it can't read.
    let c_b: Collection<u64, String> = Collection::open(e, key_b, "k", 1).unwrap();
    let r = e.begin_read().unwrap();
    assert!(matches!(c_b.get(&r, &1).unwrap_err(), StoreError::Crypto(_)));
}

fn open_missing_collection_errors(e: &impl KvEngine) {
    let res = Collection::<u64, String>::open(e, warm(), "never-created", 1);
    assert!(matches!(res.unwrap_err(), StoreError::CollectionNotFound(_)));
}

fn reserved_name_is_rejected(e: &impl KvEngine) {
    let res = Collection::<u64, String>::open_or_create(e, warm(), "_dek_wrappings", 1);
    assert!(matches!(res.unwrap_err(), StoreError::ReservedName(_)));
}

fn encrypted_range_round_trips(e: &impl KvEngine) {
    let c: Collection<u64, u64> = Collection::open_or_create(e, warm(), "nums", 1).unwrap();
    {
        let mut w = e.begin_write(Durability::Immediate).unwrap();
        for k in [3u64, 1, 2] {
            c.put(&mut w, &k, &(k * 10)).unwrap();
        }
        w.commit().unwrap();
    }
    let r = e.begin_read().unwrap();
    let got = c.range(&r, &0, &100).unwrap();
    assert_eq!(got, vec![(1, 10), (2, 20), (3, 30)]);
}

// ─── run every scenario against both engines ─────────────────────────────────

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
        round_trip,
        engine_stores_ciphertext_not_plaintext,
        tamper_at_engine_level_is_rejected,
        cold_provider_blocks_access,
        wrong_vault_key_cannot_decrypt,
        open_missing_collection_errors,
        reserved_name_is_rejected,
        encrypted_range_round_trips,
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
        round_trip,
        engine_stores_ciphertext_not_plaintext,
        tamper_at_engine_level_is_rejected,
        cold_provider_blocks_access,
        wrong_vault_key_cannot_decrypt,
        open_missing_collection_errors,
        reserved_name_is_rejected,
        encrypted_range_round_trips,
    ]
);
