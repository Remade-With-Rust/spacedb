#![forbid(unsafe_code)]
//! # spacedb-replica — SpaceDB Layer 2 (hot path)
//!
//! Live convergence between replicas of a document. M3-S1 ships the
//! **anti-entropy sync protocol** ([`SyncSession`]) over a **transport seam**
//! ([`Transport`]), plus an [`InProcessTransport`] with a partition switch so the
//! protocol is fully provable in one process.
//!
//! The protocol reconciles by exchanging state vectors and the deltas they imply
//! (built on `spacedb-crdt`'s sync primitives), so a write on one replica reaches
//! another live, and a partitioned link recovers with **zero lost writes** simply
//! by announcing again after it heals.
//!
//! ## Open-core boundary
//!
//! `Transport` is where the network lives. This crate depends only on
//! `spacedb-crdt`; MATA implements `Transport` over its iroh + relay +
//! roster-auth stack (`mata-sync`), and a self-hoster can implement it over plain
//! `iroh`. No MATA crate is referenced here.

mod error;
pub use error::{ReplicaError, ReplicaResult};

mod message;
pub use message::SyncMessage;

mod transport;
pub use transport::{connected_pair, InProcessTransport, Link, Transport};

mod session;
pub use session::{Freshness, SyncSession};

mod roles;
pub use roles::{ReplicaRole, SubsetSpec};
