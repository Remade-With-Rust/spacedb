//! M6-S3: the on-node vector index and capability-gated retrieval.

use spacedb_vector::{Match, Metric, VectorError, VectorIndex};

fn ids(matches: &[Match]) -> Vec<&str> {
    matches.iter().map(|m| m.id.as_str()).collect()
}

#[test]
fn ranks_nearest_first_by_cosine() {
    let mut idx = VectorIndex::new(3, Metric::Cosine);
    idx.insert("x", vec![1.0, 0.0, 0.0]).unwrap();
    idx.insert("y", vec![0.0, 1.0, 0.0]).unwrap();
    idx.insert("z", vec![0.0, 0.0, 1.0]).unwrap();

    let top = idx.search(&[0.9, 0.1, 0.0], 3).unwrap();
    assert_eq!(ids(&top), vec!["x", "y", "z"]); // closest to x, then y, then z
    assert!(top[0].score > top[1].score);
}

#[test]
fn returns_only_top_k() {
    let mut idx = VectorIndex::new(2, Metric::Cosine);
    for (i, v) in [[1.0, 0.0], [0.9, 0.1], [0.0, 1.0], [-1.0, 0.0]].iter().enumerate() {
        idx.insert(format!("v{i}"), v.to_vec()).unwrap();
    }
    let top = idx.search(&[1.0, 0.0], 2).unwrap();
    assert_eq!(top.len(), 2);
    assert_eq!(ids(&top), vec!["v0", "v1"]);
}

#[test]
fn euclidean_ranks_by_distance() {
    let mut idx = VectorIndex::new(1, Metric::Euclidean);
    idx.insert("near", vec![1.0]).unwrap();
    idx.insert("far", vec![5.0]).unwrap();
    let top = idx.search(&[0.0], 2).unwrap();
    assert_eq!(ids(&top), vec!["near", "far"]);
}

#[test]
fn dot_metric_rewards_magnitude_and_alignment() {
    let mut idx = VectorIndex::new(2, Metric::Dot);
    idx.insert("small", vec![1.0, 0.0]).unwrap();
    idx.insert("big", vec![10.0, 0.0]).unwrap();
    let top = idx.search(&[1.0, 0.0], 2).unwrap();
    assert_eq!(ids(&top), vec!["big", "small"]); // dot favors the larger aligned vector
}

#[test]
fn k_larger_than_corpus_returns_all() {
    let mut idx = VectorIndex::new(2, Metric::Cosine);
    idx.insert("a", vec![1.0, 0.0]).unwrap();
    idx.insert("b", vec![0.0, 1.0]).unwrap();
    let top = idx.search(&[1.0, 0.0], 100).unwrap();
    assert_eq!(top.len(), 2);
}

#[test]
fn ties_break_deterministically_by_id() {
    let mut idx = VectorIndex::new(2, Metric::Cosine);
    // identical vectors -> identical scores; order must be deterministic by id
    idx.insert("b", vec![1.0, 0.0]).unwrap();
    idx.insert("a", vec![1.0, 0.0]).unwrap();
    idx.insert("c", vec![1.0, 0.0]).unwrap();
    let top = idx.search(&[1.0, 0.0], 3).unwrap();
    assert_eq!(ids(&top), vec!["a", "b", "c"]);
}

#[test]
fn a_dimension_mismatch_is_rejected() {
    let mut idx = VectorIndex::new(3, Metric::Cosine);
    assert!(matches!(
        idx.insert("x", vec![1.0, 0.0]),
        Err(VectorError::DimMismatch { expected: 3, got: 2 })
    ));
    idx.insert("ok", vec![1.0, 0.0, 0.0]).unwrap();
    assert!(matches!(
        idx.search(&[1.0, 0.0], 1),
        Err(VectorError::DimMismatch { expected: 3, got: 2 })
    ));
}

#[test]
fn the_corpus_stays_on_node_after_a_search() {
    let mut idx = VectorIndex::new(2, Metric::Cosine);
    idx.insert("a", vec![1.0, 0.0]).unwrap();
    idx.insert("b", vec![0.0, 1.0]).unwrap();
    let _top = idx.search(&[1.0, 0.0], 1).unwrap();
    // the search returned ids+scores only; the full corpus is still held
    assert_eq!(idx.len(), 2);
    assert!(idx.remove("a"));
    assert_eq!(idx.len(), 1);
}

// ─── M5 + M6: capability-gated private retrieval ─────────────────────────────

mod gated {
    use spacedb_access::{
        authorize, AccessRequest, Capability, Decision, DenyReason, Did, Identity, MemKeyDirectory,
        Ops, RevocationSet, Scope, SignedCapability,
    };
    use spacedb_vector::{retrieve, Metric, VectorIndex};

    const NOW: u64 = 1_000_000_000;
    const FAR: u64 = 9_000_000_000;

    fn corpus() -> VectorIndex {
        let mut idx = VectorIndex::new(3, Metric::Cosine);
        idx.insert("doc-a", vec![1.0, 0.0, 0.0]).unwrap();
        idx.insert("doc-b", vec![0.0, 1.0, 0.0]).unwrap();
        idx
    }

    #[test]
    fn an_authorized_agent_retrieves_top_k_but_not_the_corpus() {
        let idx = corpus();
        let owner = Identity::generate("did:mata:owner").unwrap();
        let dir = MemKeyDirectory::new();
        dir.publish(&owner).unwrap();
        let agent = Did::from("did:agent:rag");

        // owner grants the agent a compute capability over the vector collection
        let cap = Capability::grant(owner.did().clone(), agent.clone(), Scope::Collection("vectors".into()), Ops::COMPUTE)
            .unwrap()
            .with_expiry(FAR);
        let signed = SignedCapability::sign(cap, &owner).unwrap();
        let scope = Scope::Collection("vectors".into());
        let req = AccessRequest { bearer: &agent, scope: &scope, op: Ops::COMPUTE };
        let decision = authorize(&signed, &req, &dir, NOW, &RevocationSet::new()).unwrap();
        assert!(decision.is_allowed());

        // query embedding in -> top-k out (ids + scores only)
        let matches = retrieve(&idx, &[0.95, 0.05, 0.0], 1, &decision).unwrap();
        assert_eq!(matches[0].id, "doc-a");
    }

    #[test]
    fn an_unauthorized_agent_gets_nothing() {
        let idx = corpus();
        let denied = Decision::Deny(DenyReason::NoCapability);
        assert!(matches!(
            retrieve(&idx, &[0.95, 0.05, 0.0], 1, &denied),
            Err(spacedb_vector::VectorError::Denied)
        ));
    }
}
