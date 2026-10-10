//! Deterministic instruction-count driver for the sealed row path (callgrind Ir).
//!
//! No clock, no iteration sizing: the same binary does the same work every run.
//! The printed line is the work-parity anchor — if it moves, the experiment
//! changed, not the code. Run under the `ir` profile with randomness pinned
//! (nonces only feed AES-GCM, which is constant-time, but pinning keeps the
//! count exact).

use std::sync::Arc;

use spacedb_store::{
    Collection, Compression, Durability, KeyProvider, KvEngine, MemEngine, StaticKeyProvider,
    WriteTx,
};

fn provider() -> Arc<dyn KeyProvider> {
    Arc::new(StaticKeyProvider::new([0x42; 32]))
}

fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.extend_from_slice(&state.wrapping_mul(0x2545F4914F6CDD1D).to_le_bytes());
    }
    out.truncate(len);
    out
}

fn structured_row(target: usize, i: usize) -> Vec<u8> {
    let mut s = format!(
        "{{\"id\":\"{i:08x}\",\"kind\":\"record\",\"site\":\"https://service-{}.example.com/login\",\"user\":\"user{}@example.com\",\"notes\":\"",
        i % 17,
        i
    );
    while s.len() < target.saturating_sub(2) {
        s.push_str("meeting notes: follow up on the quarterly sync; ");
    }
    s.truncate(target.saturating_sub(2));
    s.push_str("\"}");
    s.into_bytes()
}

fn fold(acc: u64, bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(acc, |h, &b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

fn main() {
    let engine = MemEngine::new();
    let mut check = 0xcbf29ce484222325u64;
    let (mut puts, mut gets, mut ranged, mut bytes) = (0u64, 0u64, 0u64, 0u64);

    // Integer keys: compressible rows (zstd path) and incompressible rows (raw path).
    let zrows: Vec<Vec<u8>> = (0..600).map(|i| structured_row(600, i)).collect();
    let rrows: Vec<Vec<u8>> = (0..300).map(|i| noise(1024, i as u64)).collect();
    let col: Collection<u64, Vec<u8>> =
        Collection::open_or_create_with(&engine, provider(), "ints", 1, Compression::default())
            .unwrap();
    {
        let mut w = engine.begin_write(Durability::Immediate).unwrap();
        for (i, row) in zrows.iter().chain(rrows.iter()).enumerate() {
            col.put(&mut w, &(i as u64), row).unwrap();
            puts += 1;
        }
        w.commit().unwrap();
    }
    {
        let r = engine.begin_read().unwrap();
        for i in 0..(zrows.len() + rrows.len()) as u64 {
            let v = col.get(&r, &i).unwrap().unwrap();
            bytes += v.len() as u64;
            check = fold(check, &v);
            gets += 1;
        }
        for (k, v) in col.range(&r, &0, &u64::MAX).unwrap() {
            check = fold(check ^ k, &v[..8]);
            ranged += 1;
        }
    }

    // String keys (doc-id shaped): exercises the escaped key codec.
    let scol: Collection<String, Vec<u8>> =
        Collection::open_or_create_with(&engine, provider(), "docs", 1, Compression::Off).unwrap();
    let ids: Vec<String> = (0..500)
        .map(|i| format!("doc/{:04}/profile-{}", i, i % 7))
        .collect();
    {
        let mut w = engine.begin_write(Durability::Immediate).unwrap();
        for (i, id) in ids.iter().enumerate() {
            scol.put(&mut w, id, &noise(200, 1000 + i as u64)).unwrap();
            puts += 1;
        }
        w.commit().unwrap();
    }
    {
        let r = engine.begin_read().unwrap();
        for id in &ids {
            let v = scol.get(&r, id).unwrap().unwrap();
            check = fold(check, &v);
            gets += 1;
        }
        for (k, v) in scol
            .range(&r, &String::new(), &"\u{10FFFF}".to_string())
            .unwrap()
        {
            check = fold(check, k.as_bytes());
            check = fold(check, &v[..4]);
            ranged += 1;
        }
    }

    println!("puts {puts} gets {gets} ranged {ranged} bytes {bytes} check {check:016x}");
}
