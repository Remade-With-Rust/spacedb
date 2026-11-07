//! Deterministic instruction-count driver: the `ir_ops` workload, ten mutations
//! per `CrdtDoc::batch` transaction. Same anchors as `ir_ops`; the Ir difference
//! is the per-transaction cost a batch amortizes.

use spacedb_crdt::CrdtDoc;

fn main() {
    fastrand::seed(7);
    let doc = CrdtDoc::new(1);
    for chunk in (0..1500).collect::<Vec<usize>>().chunks(10) {
        doc.batch(|b| {
            for &i in chunk {
                b.increment(["visits", "likes", "views"][i % 3], (i % 5) as i64 - 1);
            }
        });
    }
    for chunk in (0..300).collect::<Vec<usize>>().chunks(10) {
        doc.batch(|b| -> spacedb_crdt::CrdtResult<()> {
            for &i in chunk {
                b.set_register(&format!("field{}", i % 20), &format!("value number {i}"))?;
            }
            Ok(())
        })
        .unwrap();
    }
    for chunk in (0..200).collect::<Vec<usize>>().chunks(10) {
        doc.batch(|b| {
            for &i in chunk {
                b.set_add("tags", &format!("tag-{}", i % 50));
            }
        });
    }
    for chunk in (0..200).collect::<Vec<usize>>().chunks(10) {
        doc.batch(|b| {
            for &i in chunk {
                b.text_push("bio", if i % 2 == 0 { "hello " } else { "world " });
            }
        });
    }
    let mut check = doc.counter("visits") + doc.counter("likes") + doc.counter("views");
    for i in 0..25 {
        let v: Option<String> = doc.get_register(&format!("field{i}")).unwrap();
        check += v.map(|s| s.len() as i64).unwrap_or(0);
    }
    check += doc.text_len("bio") as i64 + doc.set_members("tags").len() as i64;
    println!("full {} check {check}", doc.encode_full().len());
}
