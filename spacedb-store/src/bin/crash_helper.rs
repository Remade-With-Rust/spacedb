//! Crash/durability test helper — driven by `tests/crash.rs`.
//!
//! It does the minimum needed to set up the crash scenario the M1 ship criterion
//! requires, then **terminates without running any destructors** so nothing gets
//! a chance to flush or cleanly close:
//!
//! 1. Durably commit a baseline write (`Immediate` durability → fsync). This MUST
//!    survive the crash.
//! 2. Open a second write and stage a value **without committing**. This must NOT
//!    survive the crash.
//! 3. Print a marker (so the parent knows we reached the crash point and didn't
//!    just panic early) and call `process::exit`, which terminates the process
//!    without unwinding the stack or dropping the open transaction / engine.
//!
//! `process::exit` is used rather than `abort` to avoid a Windows error-reporting
//! dialog; for redb's durability guarantee the two are equivalent — neither
//! flushes the uncommitted transaction, and the committed one is already fsynced.

use std::io::Write;
use std::process;

use spacedb_store::{Durability, KvEngine, RedbEngine, Table, WriteTx};

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: spacedb_crash_helper <db-path>");

    let engine = RedbEngine::open(&path).expect("open store");
    let data: Table<u64, String> = Table::new("data");

    // 1. Durable baseline.
    let mut w = engine.begin_write(Durability::Immediate).expect("begin baseline");
    data.put(&mut w, &1, &"committed".to_string()).expect("put baseline");
    w.commit().expect("commit baseline");

    // 2. Staged-but-uncommitted write that must be lost on crash.
    let mut w2 = engine.begin_write(Durability::Immediate).expect("begin uncommitted");
    data.put(&mut w2, &2, &"ghost".to_string()).expect("put uncommitted");

    // 3. Crash: leave `w2` and `engine` live and terminate without destructors.
    println!("REACHED_CRASH_POINT");
    std::io::stdout().flush().ok();
    process::exit(101);
}
