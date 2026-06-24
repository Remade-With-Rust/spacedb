//! The transport seam, and an in-process implementation.
//!
//! [`Transport`] is the open-core boundary: a byte pipe to a peer. The crate
//! ships [`InProcessTransport`] (channel-backed, with a partition switch) so the
//! whole sync protocol can be proven in a single process / in CI. MATA implements
//! `Transport` over its iroh + relay + roster-auth stack; a self-hoster can
//! implement it over plain iroh. Neither lives here — only the seam and the test
//! double do.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use crate::error::ReplicaResult;

/// A byte pipe to a peer. `send` delivers a frame; `drain` returns any frames
/// that have arrived (non-blocking). Implementations decide ordering/reliability;
/// the sync protocol tolerates dropped and reordered frames (it reconciles via
/// state vectors), so a best-effort transport is sufficient.
pub trait Transport {
    /// Deliver a frame toward the peer. A transient failure should be surfaced;
    /// a partitioned link may silently drop (the protocol heals on reconnect).
    fn send(&self, frame: &[u8]) -> ReplicaResult<()>;

    /// Return all frames that have arrived since the last call.
    fn drain(&self) -> Vec<Vec<u8>>;

    /// Whether the link to the peer is currently up. Used for honest freshness
    /// reporting (a partitioned link reads as `Partitioned`, not silently stale).
    /// Defaults to always-connected for transports that can't tell.
    fn is_connected(&self) -> bool {
        true
    }
}

/// A controllable in-process link: flip it to simulate a network partition.
#[derive(Clone)]
pub struct Link {
    connected: Arc<AtomicBool>,
}

impl Link {
    /// Drop the link — subsequent sends on both endpoints are discarded.
    pub fn partition(&self) {
        self.connected.store(false, Ordering::Relaxed);
    }

    /// Restore the link.
    pub fn heal(&self) {
        self.connected.store(true, Ordering::Relaxed);
    }

    /// Whether the link is currently connected.
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }
}

/// One endpoint of an in-process link. Created in pairs by [`connected_pair`].
pub struct InProcessTransport {
    outbound: Sender<Vec<u8>>,
    inbound: Receiver<Vec<u8>>,
    connected: Arc<AtomicBool>,
}

impl Transport for InProcessTransport {
    fn send(&self, frame: &[u8]) -> ReplicaResult<()> {
        if self.connected.load(Ordering::Relaxed) {
            // A dropped receiver (peer gone) behaves like a dead link — ignored,
            // exactly as a best-effort network transport would.
            let _ = self.outbound.send(frame.to_vec());
        }
        Ok(())
    }

    fn drain(&self) -> Vec<Vec<u8>> {
        let mut frames = Vec::new();
        while let Ok(frame) = self.inbound.try_recv() {
            frames.push(frame);
        }
        frames
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }
}

/// Create two cross-wired endpoints and the [`Link`] that controls them. Both
/// endpoints share one connectivity flag, so [`Link::partition`] cuts the link in
/// both directions (as a real partition does).
pub fn connected_pair() -> (InProcessTransport, InProcessTransport, Link) {
    let (tx_ab, rx_ab) = channel();
    let (tx_ba, rx_ba) = channel();
    let connected = Arc::new(AtomicBool::new(true));
    let a = InProcessTransport {
        outbound: tx_ab,
        inbound: rx_ba,
        connected: Arc::clone(&connected),
    };
    let b = InProcessTransport {
        outbound: tx_ba,
        inbound: rx_ab,
        connected: Arc::clone(&connected),
    };
    (a, b, Link { connected })
}
