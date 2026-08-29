//! Reloading a document, over and over, the way a restarted process does.
//!
//! A replica keeps ONE actor id for its whole life, and every restart builds a
//! new `CrdtDoc` on it. So the sequence below - encode, construct, apply,
//! write, encode again - is not a corner case; it is what a service does every
//! time it is deployed.
//!
//! It used to lose everything on the third reload. `CrdtDoc::new` passed the
//! actor id straight to `Doc::with_client_id`, so each reload was a new
//! document claiming to be the same yrs client with its clock rewound to zero:
//! the blocks it imported held clocks 0..n for that client, and the next local
//! write took a clock already spoken for. The result could not be decoded
//! again, and `yrs` reported it by dividing by zero in `find_pivot` rather than
//! returning an error.
//!
//! The failure was invisible in ordinary use because the read that follows a
//! write always looks correct - the write repaired what the load had just
//! silently emptied. These tests read BEFORE writing, which is the only place
//! the loss is visible.

use spacedb_crdt::CrdtDoc;

/// One replica's actor id, unchanged across every reload - as in production,
/// where it is derived from the node's DID.
const ACTOR: u64 = 0x9e37_79b9_7f4a_7c15;

fn reload(snapshot: &[u8], actor: u64) -> CrdtDoc {
    let doc = CrdtDoc::new(actor);
    doc.apply_update(snapshot).expect("a snapshot must reload");
    doc
}

#[test]
fn a_register_survives_repeated_reloads() {
    let mut snapshot = {
        let doc = CrdtDoc::new(ACTOR);
        doc.set_register("email", &"someone@example.com".to_string())
            .unwrap();
        doc.encode_full()
    };

    for cycle in 2..=25u32 {
        let doc = reload(&snapshot, ACTOR);

        // Read first. A read after the write below always looks right, because
        // the write repairs what a failed load emptied.
        let email: Option<String> = doc.get_register("email").unwrap();
        assert_eq!(
            email.as_deref(),
            Some("someone@example.com"),
            "reload {cycle}: the value written before the first reload is gone"
        );
        if cycle > 2 {
            let mark: Option<u32> = doc.get_register("mark").unwrap();
            assert_eq!(
                mark,
                Some(cycle - 1),
                "reload {cycle}: what reload {} wrote is gone",
                cycle - 1
            );
        }

        doc.set_register("mark", &cycle).unwrap();
        snapshot = doc.encode_full();
    }
}

/// A counter is what a balance is made of, and losing its state does not
/// corrupt it - it silently resets it to zero.
#[test]
fn a_counter_survives_repeated_reloads() {
    let mut snapshot = {
        let doc = CrdtDoc::new(ACTOR);
        doc.increment("credited", 100);
        doc.encode_full()
    };

    for cycle in 2..=25i64 {
        let doc = reload(&snapshot, ACTOR);
        assert_eq!(
            doc.counter("credited"),
            (cycle - 1) * 100,
            "reload {cycle}: increments from earlier reloads went missing"
        );
        doc.increment("credited", 100);
        snapshot = doc.encode_full();
    }
}

/// Two replicas still converge, and their counter subtotals still sum.
///
/// The fix gives each document instance its own yrs client id; this is what
/// proves it did not take the actor id's job with it. Counter subtotals are
/// keyed by ACTOR, so two replicas must still merge by summation rather than
/// one overwriting the other.
#[test]
fn separate_actors_still_converge_after_reloading() {
    let (a_actor, b_actor) = (11u64, 22u64);

    let mut a_snap = {
        let a = CrdtDoc::new(a_actor);
        a.increment("credited", 50);
        a.set_register("owner", &"a".to_string()).unwrap();
        a.encode_full()
    };
    let mut b_snap = {
        let b = CrdtDoc::new(b_actor);
        b.increment("credited", 70);
        b.encode_full()
    };

    // Reload both a few times, as two long-lived replicas would.
    for _ in 0..5 {
        let a = reload(&a_snap, a_actor);
        a.increment("credited", 1);
        a_snap = a.encode_full();

        let b = reload(&b_snap, b_actor);
        b.increment("credited", 1);
        b_snap = b.encode_full();
    }

    // Then let them meet, in both directions.
    let a = reload(&a_snap, a_actor);
    a.apply_update(&b_snap).unwrap();
    let b = reload(&b_snap, b_actor);
    b.apply_update(&a_snap).unwrap();

    // 50 + 5 from A, 70 + 5 from B. Summation, not last-write-wins.
    assert_eq!(a.counter("credited"), 130);
    assert_eq!(b.counter("credited"), 130, "the replicas did not converge");

    let owner: Option<String> = b.get_register("owner").unwrap();
    assert_eq!(
        owner.as_deref(),
        Some("a"),
        "a register written by A did not reach B"
    );
}

/// The actor id is still the replica's identity after the change.
#[test]
fn the_actor_id_is_unchanged_by_reloading() {
    let doc = CrdtDoc::new(ACTOR);
    assert_eq!(doc.actor_id(), ACTOR);
    let reloaded = reload(&doc.encode_full(), ACTOR);
    assert_eq!(
        reloaded.actor_id(),
        ACTOR,
        "the actor id must survive a reload - counter keys depend on it"
    );
}
