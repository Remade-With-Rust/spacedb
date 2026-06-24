//! [`SyncSession`] — a replica that keeps a document converged with peers over a
//! [`Transport`].
//!
//! The protocol is stateless anti-entropy, which makes live propagation and
//! partition recovery the *same* mechanism:
//!
//! - [`announce`](SyncSession::announce) sends this replica's state vector.
//! - [`pump`](SyncSession::pump) processes inbound frames: a peer's state vector
//!   is answered with exactly the delta it lacks; an update is merged.
//!
//! Because every reconciliation is driven by current state vectors (not by
//! remembering what was sent), a link that was partitioned recovers by simply
//! announcing again after it heals — no special resync path, no lost writes.
//!
//! The session owns its [`CrdtDoc`]; mutate it through [`doc`](SyncSession::doc)
//! (the doc's setters take `&self`), then `announce` to propagate.

use std::cell::RefCell;

use spacedb_crdt::CrdtDoc;

use crate::error::ReplicaResult;
use crate::message::SyncMessage;
use crate::transport::Transport;

/// The honest freshness of a replica's reads relative to its peer. The app can
/// surface this so a read is never silently mistaken for current.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Freshness {
    /// Connected and caught up to the peer's last-announced frontier.
    Live,
    /// Connected, but behind the peer by `lag_ops` operations.
    Stale { lag_ops: usize },
    /// Connected, but no peer frontier has been observed yet (never reconciled).
    Unsynced,
    /// The transport reports the link to the peer is down.
    Partitioned,
}

/// A document plus the transport over which it reconciles with a peer.
pub struct SyncSession<T: Transport> {
    doc: CrdtDoc,
    transport: T,
    /// The peer's most recently announced state vector, recorded in `pump`. Used
    /// to compute convergence lag honestly (how far behind the peer we are).
    last_peer_sv: RefCell<Option<Vec<u8>>>,
}

impl<T: Transport> SyncSession<T> {
    /// Wrap `doc` and `transport` into a session.
    pub fn new(doc: CrdtDoc, transport: T) -> Self {
        Self {
            doc,
            transport,
            last_peer_sv: RefCell::new(None),
        }
    }

    /// The document. Its mutators take `&self`, so local edits go through here.
    pub fn doc(&self) -> &CrdtDoc {
        &self.doc
    }

    /// The transport, for callers that need to drive or inspect it.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Consume the session, returning the document.
    pub fn into_doc(self) -> CrdtDoc {
        self.doc
    }

    /// Announce this replica's state vector to the peer, asking for anything it
    /// has that we lack. Call after a local change, on connect, or to recover
    /// after a partition heals.
    pub fn announce(&self) -> ReplicaResult<()> {
        let frame = SyncMessage::StateVector(self.doc.state_vector()).encode();
        self.transport.send(&frame)
    }

    /// Process every inbound frame, returning how many were acted on (a peer's
    /// state vector answered, or an update merged). Returning `0` means the
    /// session is momentarily quiescent.
    pub fn pump(&self) -> ReplicaResult<usize> {
        let mut actions = 0;
        for frame in self.transport.drain() {
            match SyncMessage::decode(&frame)? {
                SyncMessage::StateVector(their_sv) => {
                    // Record the peer's frontier so we can report honest lag.
                    *self.last_peer_sv.borrow_mut() = Some(their_sv.clone());
                    let delta = self.doc.encode_update_since(&their_sv)?;
                    self.transport.send(&SyncMessage::Update(delta).encode())?;
                    actions += 1;
                }
                SyncMessage::Update(update) => {
                    self.doc.apply_update(&update)?;
                    actions += 1;
                }
            }
        }
        Ok(actions)
    }

    /// Convergence lag: how many operations the peer's last-announced frontier
    /// has that this replica has not yet applied. `0` once reconciled; `0` also
    /// before any peer frontier has been seen (use [`freshness`](Self::freshness)
    /// to distinguish that case).
    pub fn lag(&self) -> usize {
        match self.last_peer_sv.borrow().as_deref() {
            Some(sv) => self.doc.ops_behind(sv).unwrap_or(0),
            None => 0,
        }
    }

    /// Honest read freshness: partitioned if the link is down, unsynced before any
    /// reconciliation, otherwise live or stale-by-N-ops.
    pub fn freshness(&self) -> Freshness {
        if !self.transport.is_connected() {
            return Freshness::Partitioned;
        }
        match self.last_peer_sv.borrow().as_deref() {
            None => Freshness::Unsynced,
            Some(sv) => match self.doc.ops_behind(sv).unwrap_or(0) {
                0 => Freshness::Live,
                lag_ops => Freshness::Stale { lag_ops },
            },
        }
    }
}
