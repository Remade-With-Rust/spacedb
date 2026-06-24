//! The fleet: the set of homes that can hold shards, each with a store, a failure
//! domain, and a liveness flag (so a "home dying" is just a flipped bit in tests).

use crate::placement::{TargetId, TargetInfo};
use crate::shard_store::ShardStore;

/// One home in the fleet: an id, a failure domain, a shard store, and whether it
/// is currently reachable.
pub struct Node {
    pub id: TargetId,
    pub domain: String,
    store: Box<dyn ShardStore>,
    online: bool,
}

impl Node {
    /// Create an online node backed by `store`.
    pub fn new(
        id: impl Into<TargetId>,
        domain: impl Into<String>,
        store: impl ShardStore + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            domain: domain.into(),
            store: Box::new(store),
            online: true,
        }
    }

    pub fn is_online(&self) -> bool {
        self.online
    }

    /// The node's shard store.
    pub fn store(&self) -> &dyn ShardStore {
        self.store.as_ref()
    }
}

/// A set of homes. Supports killing/reviving nodes to simulate failures.
#[derive(Default)]
pub struct Fleet {
    nodes: Vec<Node>,
}

impl Fleet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a node.
    pub fn add(&mut self, node: Node) {
        self.nodes.push(node);
    }

    /// Look up a node by id.
    pub fn node(&self, id: &TargetId) -> Option<&Node> {
        self.nodes.iter().find(|n| &n.id == id)
    }

    /// Mark a node offline (simulate a home dying). Returns whether it existed.
    pub fn kill(&mut self, id: &TargetId) -> bool {
        match self.nodes.iter_mut().find(|n| &n.id == id) {
            Some(n) => {
                n.online = false;
                true
            }
            None => false,
        }
    }

    /// Mark a node back online. Returns whether it existed.
    pub fn revive(&mut self, id: &TargetId) -> bool {
        match self.nodes.iter_mut().find(|n| &n.id == id) {
            Some(n) => {
                n.online = true;
                true
            }
            None => false,
        }
    }

    /// The currently-online targets, as placement candidates.
    pub fn online_targets(&self) -> Vec<TargetInfo> {
        self.nodes
            .iter()
            .filter(|n| n.online)
            .map(|n| TargetInfo {
                id: n.id.clone(),
                domain: n.domain.clone(),
            })
            .collect()
    }

    /// Number of online nodes.
    pub fn online_count(&self) -> usize {
        self.nodes.iter().filter(|n| n.online).count()
    }
}
