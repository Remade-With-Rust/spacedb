//! M4-S2: placement (anti-affinity), distribution onto a fleet, and recovery from
//! the survivors after homes die.

use std::collections::{HashMap, HashSet};

use spacedb_durability::{
    allocate, distribute, encode_snapshot, reachable_shard_count, recover, DurabilityError, Fleet,
    MemShardStore, Node, Placement, TargetId,
};

// Six homes across three failure domains, two per domain.
const SPECS: &[(&str, &str)] = &[
    ("n0", "d0"),
    ("n1", "d0"),
    ("n2", "d1"),
    ("n3", "d1"),
    ("n4", "d2"),
    ("n5", "d2"),
];

fn fleet() -> Fleet {
    let mut f = Fleet::new();
    for (id, domain) in SPECS {
        f.add(Node::new(*id, *domain, MemShardStore::new()));
    }
    f
}

fn domain_of(id: &TargetId) -> String {
    SPECS
        .iter()
        .find(|(i, _)| *i == id.0)
        .map(|(_, d)| d.to_string())
        .unwrap()
}

fn sample(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 + 3) as u8).collect()
}

#[test]
fn allocate_spreads_shards_across_distinct_domains() {
    let fleet = fleet();
    let placement = allocate(6, &fleet.online_targets()).unwrap();

    // distinct targets, one shard each
    let distinct: HashSet<&TargetId> = placement.targets().iter().collect();
    assert_eq!(distinct.len(), 6);

    // anti-affinity: each of the 3 domains holds ceil(6/3) = 2 shards
    let mut per_domain: HashMap<String, usize> = HashMap::new();
    for target in placement.targets() {
        *per_domain.entry(domain_of(target)).or_default() += 1;
    }
    assert_eq!(per_domain.len(), 3);
    assert!(per_domain.values().all(|&c| c == 2), "spread evenly across domains");
}

#[test]
fn allocate_errors_when_too_few_targets() {
    let mut f = Fleet::new();
    f.add(Node::new("a", "d0", MemShardStore::new()));
    f.add(Node::new("b", "d0", MemShardStore::new()));
    let err = allocate(5, &f.online_targets()).unwrap_err();
    assert!(matches!(err, DurabilityError::NotEnoughTargets { have: 2, need: 5 }));
}

#[test]
fn distribute_then_recover_round_trips() {
    let data = sample(1500);
    let (manifest, shards) = encode_snapshot(&data, 4, 2).unwrap();
    let fleet = fleet();
    let placement = allocate(manifest.total_shards(), &fleet.online_targets()).unwrap();

    distribute(&manifest, &shards, &placement, &fleet).unwrap();
    assert_eq!(reachable_shard_count(&manifest, &placement, &fleet).unwrap(), 6);
    assert_eq!(recover(&manifest, &placement, &fleet).unwrap(), data);
}

#[test]
fn recovers_after_losing_up_to_parity_homes() {
    let data = sample(1500);
    let (manifest, shards) = encode_snapshot(&data, 4, 2).unwrap(); // tolerate 2
    let mut fleet = fleet();
    let placement = allocate(6, &fleet.online_targets()).unwrap();
    distribute(&manifest, &shards, &placement, &fleet).unwrap();

    // kill two homes (different domains) — still recoverable
    fleet.kill(&"n0".into());
    fleet.kill(&"n2".into());
    assert_eq!(reachable_shard_count(&manifest, &placement, &fleet).unwrap(), 4);
    assert_eq!(recover(&manifest, &placement, &fleet).unwrap(), data);
}

#[test]
fn fails_when_more_than_parity_homes_die() {
    let data = sample(1500);
    let (manifest, shards) = encode_snapshot(&data, 4, 2).unwrap();
    let mut fleet = fleet();
    let placement = allocate(6, &fleet.online_targets()).unwrap();
    distribute(&manifest, &shards, &placement, &fleet).unwrap();

    fleet.kill(&"n0".into());
    fleet.kill(&"n1".into());
    fleet.kill(&"n2".into()); // 3 dead > parity(2)
    assert_eq!(reachable_shard_count(&manifest, &placement, &fleet).unwrap(), 3);
    let err = recover(&manifest, &placement, &fleet).unwrap_err();
    assert!(matches!(
        err,
        DurabilityError::InsufficientShards { have: 3, need: 4 }
    ));
}

#[test]
fn a_whole_domain_failure_is_survivable() {
    // The anti-affinity payoff: each domain holds exactly `parity` shards, so
    // losing an entire failure domain stays recoverable.
    let data = sample(2000);
    let (manifest, shards) = encode_snapshot(&data, 4, 2).unwrap();
    let mut fleet = fleet();
    let placement = allocate(6, &fleet.online_targets()).unwrap();
    distribute(&manifest, &shards, &placement, &fleet).unwrap();

    // kill the whole d0 domain (n0, n1) — 2 shards lost == parity
    fleet.kill(&"n0".into());
    fleet.kill(&"n1".into());
    assert_eq!(recover(&manifest, &placement, &fleet).unwrap(), data);
}

#[test]
fn distribute_to_an_offline_target_errors() {
    let data = sample(500);
    let (manifest, shards) = encode_snapshot(&data, 4, 2).unwrap();
    let mut fleet = fleet();
    let placement = allocate(6, &fleet.online_targets()).unwrap();
    // kill a placed target before distributing
    let dead = placement.target_for(0).unwrap().clone();
    fleet.kill(&dead);
    let err = distribute(&manifest, &shards, &placement, &fleet).unwrap_err();
    assert!(matches!(err, DurabilityError::TargetOffline(_)));
}

#[test]
fn placement_record_round_trips_through_its_codec() {
    let fleet = fleet();
    let placement = allocate(6, &fleet.online_targets()).unwrap();
    let bytes = placement.encode().unwrap();
    assert_eq!(Placement::decode(&bytes).unwrap(), placement);
}
