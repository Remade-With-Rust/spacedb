//! The M1 ship criterion: a typed, encrypted, crash-safe store **proven by a
//! kill-during-write durability test that recovers cleanly**.
//!
//! A child process commits a baseline, opens an uncommitted write, then dies
//! without running destructors (see `src/bin/crash_helper.rs`). This process then
//! reopens the same file and asserts that redb recovered to exactly the last
//! committed state: the committed write survived, the uncommitted one did not, and
//! the store is fully usable again. This is a redb-only property — the in-memory
//! engine has no durability to test.

use std::process::Command;

use spacedb_store::{Durability, KvEngine, RedbEngine, Table, WriteTx};

#[test]
fn store_recovers_to_last_committed_state_after_a_crash() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("crash.redb");

    // --- crash the child mid-session ---
    let out = Command::new(env!("CARGO_BIN_EXE_spacedb_crash_helper"))
        .arg(&path)
        .output()
        .expect("spawn crash helper");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("REACHED_CRASH_POINT"),
        "helper did not reach the crash point (it may have failed early). stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let data: Table<u64, String> = Table::new("data");

    // --- reopen: must recover cleanly to the last committed state ---
    let engine = RedbEngine::open(&path).expect("store must reopen after a crash");
    {
        let r = engine.begin_read().unwrap();
        assert_eq!(
            data.get(&r, &1).unwrap(),
            Some("committed".to_string()),
            "the committed write must survive the crash"
        );
        assert_eq!(
            data.get(&r, &2).unwrap(),
            None,
            "the uncommitted write must NOT survive the crash"
        );
    }

    // --- the recovered store is fully writable again ---
    {
        let mut w = engine.begin_write(Durability::Immediate).unwrap();
        data.put(&mut w, &3, &"after-recovery".to_string()).unwrap();
        w.commit().unwrap();
    }
    drop(engine);

    // --- reopen once more: baseline + post-recovery write present, ghost gone ---
    let engine2 = RedbEngine::open(&path).expect("reopen after post-recovery write");
    let r = engine2.begin_read().unwrap();
    assert_eq!(data.get(&r, &1).unwrap(), Some("committed".to_string()));
    assert_eq!(data.get(&r, &2).unwrap(), None);
    assert_eq!(data.get(&r, &3).unwrap(), Some("after-recovery".to_string()));
}
