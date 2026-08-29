//! Regression guard for the relay re-encode defect.
//!
//! `encode_update_since` called on a third "relay" doc that sits between two
//! replicas (A → relay → B) can silently **drop a record** for certain actor-id
//! orderings — the very orderings hash-derived device ids produce. A single
//! direct merge is fine; the loss needs the re-encoding relay plus ≥2 rounds.
//! (See the warning on `CrdtDoc::encode_update_since`.)
//!
//! The supported sync pattern therefore never re-encodes: each replica ships its
//! raw LOCAL update bytes ([`CrdtDoc::take_local_updates`]) to an append-only
//! log, and peers apply them **verbatim and idempotently**. A relay keeps a
//! queryable copy by replaying the same log. This test reproduces the exact
//! defect topology (two devices, a relayed log, two rounds) with the adversarial
//! hash-derived actors and asserts conflict-free convergence.
//!
//! This guard previously lived only in a downstream consumer's test suite; it
//! belongs here, so it travels with the code it protects.

use spacedb_crdt::CrdtDoc;

/// A stable CRDT actor id derived from a device id — the id shape that
/// triggered the original defect (toy ids 1/2/3 passed; hash-derived ids failed).
fn actor_from(device_id: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    device_id.hash(&mut h);
    h.finish() | 1
}

#[test]
fn raw_local_updates_through_a_log_converge() {
    let a = CrdtDoc::new(actor_from("laptop"));
    a.set_register("c:n1", &"v1".to_string()).unwrap();
    let b = CrdtDoc::new(actor_from("phone"));
    b.set_register("c:n2", &"v2".to_string()).unwrap();

    let mut log: Vec<Vec<u8>> = Vec::new();
    // One sync round for `doc`: apply the log tail past `applied`, then append
    // this replica's new local updates — the wire pattern every consumer uses.
    let sync = |doc: &CrdtDoc, applied: &mut usize, log: &mut Vec<Vec<u8>>| {
        for upd in &log[*applied..] {
            doc.apply_update(upd).unwrap();
        }
        log.extend(doc.take_local_updates());
        *applied = log.len();
    };
    let (mut a_applied, mut b_applied) = (0usize, 0usize);
    sync(&a, &mut a_applied, &mut log);
    sync(&b, &mut b_applied, &mut log);
    sync(&a, &mut a_applied, &mut log);
    sync(&b, &mut b_applied, &mut log);

    for (d, who) in [(&a, "A"), (&b, "B")] {
        assert_eq!(
            d.get_register::<String>("c:n1").unwrap(),
            Some("v1".into()),
            "{who} missing n1"
        );
        assert_eq!(
            d.get_register::<String>("c:n2").unwrap(),
            Some("v2".into()),
            "{who} missing n2"
        );
    }
}
