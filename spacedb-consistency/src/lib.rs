#![forbid(unsafe_code)]
//! # spacedb-consistency — SpaceDB Layer 3 (consistency tiers)
//!
//! Consistency is a **per-field choice**, declared in the schema, because in a
//! partition-prone world one global setting is always wrong. Three tiers:
//!
//! - [`Tier::Convergent`] — CRDT, the default: always available, auto-merging.
//! - [`Tier::Causal`] — session read-your-writes / monotonic reads via a
//!   [`CausalSession`], cheap and partition-tolerant, no consensus.
//! - [`Tier::Strong`] — linearizable, a quorum that **fails safe** under partition
//!   (M7-S2).
//!
//! And the honesty contract: every op returns the [`Outcome`] it actually achieved
//! — `Committed{tier}` / `Local` / `Stale{lag}` / `Unavailable{reason}` — so an app
//! can never mistake a local-only write for a durable one or a lagging read for a
//! current one.
//!
//! M7-S1 ships the tier annotations, the honesty contract, and the Causal+ tier;
//! the Strong quorum tier is S2. Open-core (MIT).

mod tier;
pub use tier::{ConsistencySchema, Tier};

mod outcome;
pub use outcome::{Outcome, UnavailableReason};

mod causal;
pub use causal::CausalSession;

mod strong;
pub use strong::{QuorumGroup, RejectReason, StrongResult};
