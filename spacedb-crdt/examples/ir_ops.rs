//! Deterministic instruction-count driver: one document, every CRDT type, each
//! mutation in its own transaction (the per-op API). `ir_batch` does the same
//! work through `CrdtDoc::batch`; the two print identical anchors, and the Ir
//! difference is what one transaction per mutation costs.

use spacedb_crdt::CrdtDoc;

fn main() {
    fastrand::seed(7);
    let doc = CrdtDoc::new(1);
    for i in 0..1500 {
        doc.increment(["visits", "likes", "views"][i % 3], (i % 5) as i64 - 1);
    }
    for i in 0..300 {
        doc.set_register(&format!("field{}", i % 20), &format!("value number {i}"))
            .unwrap();
    }
    for i in 0..200 {
        doc.set_add("tags", &format!("tag-{}", i % 50));
    }
    for i in 0..200 {
        doc.text_push("bio", if i % 2 == 0 { "hello " } else { "world " });
    }
    let mut check = doc.counter("visits") + doc.counter("likes") + doc.counter("views");
    for i in 0..25 {
        let v: Option<String> = doc.get_register(&format!("field{i}")).unwrap();
        check += v.map(|s| s.len() as i64).unwrap_or(0);
    }
    check += doc.text_len("bio") as i64 + doc.set_members("tags").len() as i64;
    println!("full {} check {check}", doc.encode_full().len());
}
