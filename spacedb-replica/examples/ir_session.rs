//! Deterministic instruction-count driver for `SyncSession` over the in-process
//! transport: two replicas write, announce and pump to quiescence, repeatedly.
//! The converged counters are the anchor.

use spacedb_crdt::CrdtDoc;
use spacedb_replica::{connected_pair, SyncSession};

fn main() {
    fastrand::seed(3);
    let (ta, tb, _link) = connected_pair();
    let a = SyncSession::new(CrdtDoc::new(10), ta);
    let b = SyncSession::new(CrdtDoc::new(20), tb);
    let mut pumped = 0usize;
    for round in 0..200 {
        a.doc().increment("n", 1);
        if round % 3 == 0 {
            b.doc().increment("n", 2);
        }
        a.announce().unwrap();
        b.announce().unwrap();
        loop {
            let progressed = a.pump().unwrap() + b.pump().unwrap();
            pumped += progressed;
            if progressed == 0 {
                break;
            }
        }
    }
    println!("a {} b {} pumped {pumped}", a.doc().counter("n"), b.doc().counter("n"));
}
