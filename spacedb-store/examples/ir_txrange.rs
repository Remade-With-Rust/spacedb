//! Deterministic instruction-count driver for a range scan inside a write
//! transaction (`MemWriteTx::range_raw`): committed rows merged with the
//! transaction's own puts, overwrites and deletes. The checksum is the anchor.

use spacedb_store::{Durability, KvEngine, MemEngine, Table, WriteTx};

fn main() {
    let engine = MemEngine::new();
    let t: Table<u64, u64> = Table::new("rows");
    {
        let mut w = engine.begin_write(Durability::Immediate).unwrap();
        for k in (0..2000u64).step_by(2) {
            t.put(&mut w, &k, &k).unwrap();
        }
        w.commit().unwrap();
    }
    let mut w = engine.begin_write(Durability::Immediate).unwrap();
    for k in (1..2000u64).step_by(4) {
        t.put(&mut w, &k, &(k * 10)).unwrap(); // interleaved new keys
    }
    for k in (0..2000u64).step_by(8) {
        t.put(&mut w, &k, &(k + 7)).unwrap(); // overwrites
    }
    for k in (4..2000u64).step_by(16) {
        t.delete(&mut w, &k).unwrap(); // deletes
    }
    let (mut rows, mut check) = (0u64, 0u64);
    for lo in (0..2000u64).step_by(100) {
        for (k, v) in t.range(&w, &lo, &(lo + 400)).unwrap() {
            rows += 1;
            check = check.wrapping_mul(31).wrapping_add(k ^ v);
        }
    }
    println!("rows {rows} check {check}");
}
