//! M4-S3: replica-count health and the repair loop — the "a home dies, the data
//! restores itself elsewhere, no human" ship criterion.

use spacedb_durability::{
    allocate, distribute, encode_snapshot, health, recover, repair, DurabilityError, Fleet,
    HealthStatus, MemShardStore, Node,
};

fn base_fleet() -> Fleet {
    let mut f = Fleet::new();
    for (id, domain) in [
        ("n0", "d0"),
        ("n1", "d0"),
        ("n2", "d1"),
        ("n3", "d1"),
        ("n4", "d2"),
        ("n5", "d2"),
    ] {
        f.add(Node::new(id, domain, MemShardStore::new()));
    }
    f
}

fn sample(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 + 3) as u8).collect()
}

#[test]
fn health_transitions_healthy_degraded_lost() {
    let data = sample(1200);
    let (manifest, shards) = encode_snapshot(&data, 4, 2).unwrap();
    let mut fleet = base_fleet();
    let placement = allocate(6, &fleet.online_targets()).unwrap();
    distribute(&manifest, &shards, &placement, &fleet).unwrap();

    let h = health(&manifest, &placement, &fleet).unwrap();
    assert_eq!(h.status, HealthStatus::Healthy);
    assert_eq!(h.reachable, 6);
    assert_eq!(h.slack(), 2);

    fleet.kill(&"n0".into());
    let h = health(&manifest, &placement, &fleet).unwrap();
    assert_eq!(h.status, HealthStatus::Degraded { missing: 1 });
    assert_eq!(h.slack(), 1);
    assert!(h.is_repairable());

    fleet.kill(&"n1".into());
    let h = health(&manifest, &placement, &fleet).unwrap();
    assert_eq!(h.status, HealthStatus::Degraded { missing: 2 });
    assert_eq!(h.slack(), 0);

    fleet.kill(&"n2".into()); // reachable 3 < k=4
    let h = health(&manifest, &placement, &fleet).unwrap();
    assert_eq!(h.status, HealthStatus::Lost);
    assert!(!h.is_repairable());
}

#[test]
fn repair_is_a_noop_when_healthy() {
    let data = sample(800);
    let (manifest, shards) = encode_snapshot(&data, 4, 2).unwrap();
    let fleet = base_fleet();
    let placement = allocate(6, &fleet.online_targets()).unwrap();
    distribute(&manifest, &shards, &placement, &fleet).unwrap();

    let report = repair(&manifest, &placement, &fleet).unwrap();
    assert!(report.repaired_shards.is_empty());
    assert_eq!(report.new_placement, placement);
}

#[test]
fn repair_restores_the_replica_count_onto_fresh_homes() {
    let data = sample(2048);
    let (manifest, shards) = encode_snapshot(&data, 4, 2).unwrap();
    let mut fleet = base_fleet();
    let placement = allocate(6, &fleet.online_targets()).unwrap();
    distribute(&manifest, &shards, &placement, &fleet).unwrap();

    // two homes (in different domains) die
    fleet.kill(&"n0".into()); // holds shard 0
    fleet.kill(&"n2".into()); // holds shard 1
    assert_eq!(
        health(&manifest, &placement, &fleet).unwrap().status,
        HealthStatus::Degraded { missing: 2 }
    );

    // operator (Falcon) brings two fresh homes online and runs repair
    fleet.add(Node::new("n6", "d0", MemShardStore::new()));
    fleet.add(Node::new("n7", "d1", MemShardStore::new()));
    let report = repair(&manifest, &placement, &fleet).unwrap();
    assert_eq!(report.repaired_shards, vec![0, 1]);

    // replica count restored, and the snapshot still reconstructs from the new map
    let restored = health(&manifest, &report.new_placement, &fleet).unwrap();
    assert_eq!(restored.status, HealthStatus::Healthy);
    assert_eq!(restored.reachable, 6);
    assert_eq!(recover(&manifest, &report.new_placement, &fleet).unwrap(), data);
}

#[test]
fn fault_tolerance_is_restored_after_repair() {
    // After repair, the snapshot can again survive `parity` fresh losses.
    let data = sample(2048);
    let (manifest, shards) = encode_snapshot(&data, 4, 2).unwrap();
    let mut fleet = base_fleet();
    let placement = allocate(6, &fleet.online_targets()).unwrap();
    distribute(&manifest, &shards, &placement, &fleet).unwrap();

    fleet.kill(&"n0".into());
    fleet.kill(&"n2".into());
    fleet.add(Node::new("n6", "d0", MemShardStore::new()));
    fleet.add(Node::new("n7", "d1", MemShardStore::new()));
    let report = repair(&manifest, &placement, &fleet).unwrap();

    // now kill two MORE of the (restored) reachable homes — still recoverable
    fleet.kill(&"n4".into()); // shard 2
    fleet.kill(&"n1".into()); // shard 3
    assert_eq!(
        recover(&manifest, &report.new_placement, &fleet).unwrap(),
        data,
        "repair restored full fault tolerance"
    );
}

#[test]
fn repair_fails_when_too_few_shards_survive() {
    let data = sample(1000);
    let (manifest, shards) = encode_snapshot(&data, 4, 2).unwrap();
    let mut fleet = base_fleet();
    let placement = allocate(6, &fleet.online_targets()).unwrap();
    distribute(&manifest, &shards, &placement, &fleet).unwrap();

    // three homes die — only 3 < k=4 survive; even with spares, nothing to rebuild from
    fleet.kill(&"n0".into());
    fleet.kill(&"n1".into());
    fleet.kill(&"n2".into());
    fleet.add(Node::new("n6", "d0", MemShardStore::new()));
    let err = repair(&manifest, &placement, &fleet).unwrap_err();
    assert!(matches!(
        err,
        DurabilityError::InsufficientShards { have: 3, need: 4 }
    ));
}

#[test]
fn repair_fails_without_fresh_homes() {
    let data = sample(1000);
    let (manifest, shards) = encode_snapshot(&data, 4, 2).unwrap();
    let mut fleet = base_fleet();
    let placement = allocate(6, &fleet.online_targets()).unwrap();
    distribute(&manifest, &shards, &placement, &fleet).unwrap();

    // two homes die, but no spare homes are available to receive the regenerated shards
    fleet.kill(&"n0".into());
    fleet.kill(&"n2".into());
    let err = repair(&manifest, &placement, &fleet).unwrap_err();
    assert!(matches!(
        err,
        DurabilityError::NotEnoughTargets { have: 0, need: 2 }
    ));
}
