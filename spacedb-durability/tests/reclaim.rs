//! M4-S4: reclamation — drop the surplus copies a transient failure + repair
//! leaves behind, so a rejoined home doesn't run up the storage bill.

use spacedb_durability::{
    allocate, distribute, encode_snapshot, reclaim, recover, repair, surplus_shard_count, Fleet,
    MemShardStore, Node, TargetId,
};

/// Eight homes across four domains — more than the six shards, so repair always
/// has a fresh home to regenerate onto.
fn base_fleet() -> Fleet {
    let mut f = Fleet::new();
    for (id, domain) in [
        ("n0", "d0"), ("n1", "d0"),
        ("n2", "d1"), ("n3", "d1"),
        ("n4", "d2"), ("n5", "d2"),
        ("n6", "d3"), ("n7", "d3"),
    ] {
        f.add(Node::new(id, domain, MemShardStore::new()));
    }
    f
}

fn sample(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 + 3) as u8).collect()
}

#[test]
fn a_rejoined_home_leaves_a_reclaimable_orphan_that_reclaim_drops() {
    let data = sample(1200);
    let (manifest, shards) = encode_snapshot(&data, 4, 2).unwrap(); // 6 shards
    let mut fleet = base_fleet();
    let placement = allocate(6, &fleet.online_targets()).unwrap();
    distribute(&manifest, &shards, &placement, &fleet).unwrap();

    // a freshly-distributed snapshot has no surplus
    assert_eq!(surplus_shard_count(&manifest, &placement, &fleet).unwrap(), 0);
    assert!(reclaim(&manifest, &placement, &fleet).unwrap().is_empty());

    // a home holding shard 0 dies; repair regenerates shard 0 onto a fresh home
    let dead: TargetId = placement.targets()[0].clone();
    fleet.kill(&dead);
    let report = repair(&manifest, &placement, &fleet).unwrap();
    let new_placement = report.new_placement;
    assert_eq!(report.repaired_shards, vec![0]);
    assert_ne!(new_placement.targets()[0], dead); // shard 0 moved off the dead home

    // the dead home rejoins — still holding the original shard 0 bytes
    fleet.revive(&dead);
    assert_eq!(
        surplus_shard_count(&manifest, &new_placement, &fleet).unwrap(),
        1,
        "the rejoined home is now a surplus copy"
    );

    // reclaim drops exactly that orphan
    let r = reclaim(&manifest, &new_placement, &fleet).unwrap();
    assert_eq!(r.reclaimed.len(), 1);
    assert_eq!(r.reclaimed[0].target, dead);
    assert_eq!(r.reclaimed[0].shard_index, 0);
    assert!(r.bytes_reclaimed > 0);

    // surplus is gone, reclaim is idempotent, and the data still reconstructs
    assert_eq!(surplus_shard_count(&manifest, &new_placement, &fleet).unwrap(), 0);
    assert!(reclaim(&manifest, &new_placement, &fleet).unwrap().is_empty());
    assert_eq!(recover(&manifest, &new_placement, &fleet).unwrap(), data);
}

#[test]
fn an_orphan_is_kept_while_its_live_copy_is_down() {
    let data = sample(900);
    let (manifest, shards) = encode_snapshot(&data, 4, 2).unwrap();
    let mut fleet = base_fleet();
    let placement = allocate(6, &fleet.online_targets()).unwrap();
    distribute(&manifest, &shards, &placement, &fleet).unwrap();

    let dead: TargetId = placement.targets()[0].clone();
    fleet.kill(&dead);
    let new_placement = repair(&manifest, &placement, &fleet).unwrap().new_placement;
    fleet.revive(&dead);

    // now the *repaired* (live) copy of shard 0 goes offline
    let live = new_placement.targets()[0].clone();
    fleet.kill(&live);

    // the orphan must be kept: it's the cheap re-adoption shortcut for repair,
    // not safe to delete while the placement's own copy is unreachable
    let r = reclaim(&manifest, &new_placement, &fleet).unwrap();
    assert!(r.is_empty(), "must not reclaim while the live copy is down");
    assert_eq!(surplus_shard_count(&manifest, &new_placement, &fleet).unwrap(), 1);

    // once the live copy returns, the orphan becomes reclaimable again
    fleet.revive(&live);
    let r = reclaim(&manifest, &new_placement, &fleet).unwrap();
    assert_eq!(r.reclaimed.len(), 1);
    assert_eq!(r.reclaimed[0].target, dead);
}
