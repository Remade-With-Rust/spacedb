//! The schema — every field declares its **CRDT type** (how it merges) and its
//! **consistency tier** (how strong a guarantee it carries). This is the single
//! place a developer encodes "this is a counter that auto-merges" vs "this username
//! must be globally unique", and the rest of the SDK routes accordingly.

use std::collections::HashMap;

use spacedb_consistency::Tier;

/// The CRDT a field is represented by — its merge semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrdtType {
    /// Last-writer-wins register (a scalar value).
    Register,
    /// PN-counter (add/subtract, commutes).
    Counter,
    /// Collaborative text.
    Text,
    /// Add-wins observed-remove set.
    Set,
}

impl CrdtType {
    pub fn name(&self) -> &'static str {
        match self {
            CrdtType::Register => "register",
            CrdtType::Counter => "counter",
            CrdtType::Text => "text",
            CrdtType::Set => "set",
        }
    }
}

/// A field's representation and guarantee.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldSpec {
    pub crdt: CrdtType,
    pub tier: Tier,
}

/// The fields of a collection.
#[derive(Clone, Debug)]
pub struct Schema {
    collection: String,
    fields: HashMap<String, FieldSpec>,
}

impl Schema {
    pub fn new(collection: impl Into<String>) -> Self {
        Self {
            collection: collection.into(),
            fields: HashMap::new(),
        }
    }

    /// Declare a field (builder style). A `Register`/`Counter`/`Text`/`Set` at a
    /// `Convergent`/`Causal`/`Strong` tier.
    pub fn field(mut self, name: impl Into<String>, crdt: CrdtType, tier: Tier) -> Self {
        self.fields.insert(name.into(), FieldSpec { crdt, tier });
        self
    }

    pub fn collection(&self) -> &str {
        &self.collection
    }

    pub fn spec(&self, field: &str) -> Option<FieldSpec> {
        self.fields.get(field).copied()
    }
}
