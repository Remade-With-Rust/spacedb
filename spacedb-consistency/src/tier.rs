//! Consistency tiers and the per-field schema that selects them.
//!
//! In a partition-prone world one global consistency setting is always wrong:
//! most data wants availability, a little wants linearizability, and only the
//! developer knows which is which. So consistency is a **per-field choice**,
//! declared in the schema.

use std::collections::HashMap;

/// The consistency a field is served at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// CRDT, the default for ~95% of data: always writable offline, auto-merging,
    /// never blocks. Content, profiles, tags, feeds, tallies.
    Convergent,
    /// Session causal+: read-your-writes and monotonic reads via a causal token
    /// over state vectors — cheap and partition-tolerant, no consensus.
    Causal,
    /// Linearizable, opt-in and deliberately expensive: uniqueness, non-negative
    /// invariants, money. A quorum that **fails safe** under partition.
    Strong,
}

/// Which tier each field is served at; everything defaults to [`Tier::Convergent`].
#[derive(Clone, Debug)]
pub struct ConsistencySchema {
    default: Tier,
    fields: HashMap<String, Tier>,
}

impl Default for ConsistencySchema {
    fn default() -> Self {
        Self {
            default: Tier::Convergent,
            fields: HashMap::new(),
        }
    }
}

impl ConsistencySchema {
    /// A schema where every field defaults to [`Tier::Convergent`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Annotate `field` with `tier` (builder style).
    pub fn with_field(mut self, field: impl Into<String>, tier: Tier) -> Self {
        self.fields.insert(field.into(), tier);
        self
    }

    /// The tier `field` is served at — its annotation, or the default.
    pub fn tier_of(&self, field: &str) -> Tier {
        self.fields.get(field).copied().unwrap_or(self.default)
    }

    /// The default tier.
    pub fn default_tier(&self) -> Tier {
        self.default
    }
}
