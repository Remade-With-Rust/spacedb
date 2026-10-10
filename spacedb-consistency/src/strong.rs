//! The Strong (linearizable) tier — a quorum that **fails safe** under partition.
//!
//! Only the genuine minority of data that CRDTs cannot express needs this:
//! **uniqueness** (one username, one seat), **non-negative invariants** (don't
//! oversell), **money**. The mechanism is a per-key **quorum** of members, each
//! holding a versioned register; a write is a compare-and-set that commits only if
//! a **majority** agrees, so:
//!
//! - Concurrent writers race on the version: exactly one wins; the loser is
//!   refused, never double-committed (no two usernames, no oversold seat).
//! - **Under partition the quorum is unreachable → the op returns
//!   [`StrongResult::Unavailable`] and commits nothing.** It fails safe, not open —
//!   a minority side can never diverge, because only a majority side can commit
//!   and there is at most one majority side.
//!
//! This is the in-process consensus core (2-of-3 happy path + the fail-safe
//! partition behaviour, which is Phase-1 scope). The scheduler-placed, anti-affine
//! membership and the cross-host transport are the M3/M4 seams; production
//! reconfiguration-under-churn is Phase 2.

use std::collections::HashMap;

use crate::outcome::{Outcome, UnavailableReason};
use crate::tier::Tier;

/// Why a strong op was refused by the invariant (the quorum *was* reached).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RejectReason {
    /// A uniqueness key is already owned.
    AlreadyClaimed,
    /// A non-negative resource is exhausted.
    Exhausted,
    /// A concurrent writer already advanced the version (this CAS lost the race).
    VersionConflict,
}

/// The result of a strong op. `Committed`/`Rejected` mean the quorum was reached
/// and gave a definitive, linearizable answer; `Unavailable` means it was not
/// reached and **nothing was committed**.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StrongResult {
    Committed,
    Rejected(RejectReason),
    Unavailable(UnavailableReason),
}

impl StrongResult {
    pub fn is_committed(&self) -> bool {
        matches!(self, StrongResult::Committed)
    }

    /// Whether the quorum was reached (a definitive answer, committed or refused).
    pub fn is_linearizable(&self) -> bool {
        !matches!(self, StrongResult::Unavailable(_))
    }

    /// Map to the honesty contract's consistency level: a reached quorum is
    /// `Committed(Strong)` (linearizably decided); otherwise `Unavailable`.
    pub fn consistency(&self) -> Outcome {
        match self {
            StrongResult::Committed | StrongResult::Rejected(_) => Outcome::Committed(Tier::Strong),
            StrongResult::Unavailable(reason) => Outcome::Unavailable(*reason),
        }
    }
}

#[derive(Clone, Debug)]
struct Member {
    id: String,
    online: bool,
    /// key → (value, version).
    store: HashMap<String, (Vec<u8>, u64)>,
}

/// A quorum of members holding versioned registers for strong-tier keys.
pub struct QuorumGroup {
    members: Vec<Member>,
}

impl QuorumGroup {
    /// A group of the given members, all initially reachable.
    pub fn new<I, S>(member_ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let members = member_ids
            .into_iter()
            .map(|id| Member {
                id: id.into(),
                online: true,
                store: HashMap::new(),
            })
            .collect();
        Self { members }
    }

    pub fn size(&self) -> usize {
        self.members.len()
    }

    /// The number of members that must agree (a strict majority).
    pub fn majority(&self) -> usize {
        self.members.len() / 2 + 1
    }

    pub fn online_count(&self) -> usize {
        self.members.iter().filter(|m| m.online).count()
    }

    /// Take a member offline (simulate it being on the far side of a partition).
    pub fn partition(&mut self, member_id: &str) -> bool {
        self.set_online(member_id, false)
    }

    /// Bring a member back online.
    pub fn heal(&mut self, member_id: &str) -> bool {
        self.set_online(member_id, true)
    }

    fn set_online(&mut self, member_id: &str, online: bool) -> bool {
        match self.members.iter_mut().find(|m| m.id == member_id) {
            Some(m) => {
                m.online = online;
                true
            }
            None => false,
        }
    }

    /// [`read`](Self::read), borrowing the winning value instead of cloning it.
    fn latest(&self, key: &str) -> Result<(Option<&[u8]>, u64), UnavailableReason> {
        if self.online_count() < self.majority() {
            return Err(UnavailableReason::QuorumUnreachable);
        }
        let best = self
            .members
            .iter()
            .filter(|m| m.online)
            .filter_map(|m| m.store.get(key))
            .max_by_key(|(_, version)| *version);
        Ok(match best {
            Some((value, version)) => (Some(value.as_slice()), *version),
            None => (None, 0),
        })
    }

    /// Read the latest committed `(value, version)` for `key` from a quorum. The
    /// highest version across any reachable majority is the latest committed
    /// value (any two majorities overlap). Errors if a majority isn't reachable.
    pub fn read(&self, key: &str) -> Result<(Option<Vec<u8>>, u64), UnavailableReason> {
        let (value, version) = self.latest(key)?;
        Ok((value.map(<[u8]>::to_vec), version))
    }

    /// Compare-and-set: commit `new_value` at `expected_version + 1` to a majority,
    /// but only if the current committed version is still `expected_version`.
    /// `Unavailable` if no majority is reachable (nothing is written); `Rejected`
    /// if a concurrent writer already advanced the version.
    pub fn cas(&mut self, key: &str, expected_version: u64, new_value: Vec<u8>) -> StrongResult {
        if self.online_count() < self.majority() {
            return StrongResult::Unavailable(UnavailableReason::QuorumUnreachable);
        }
        let current = self
            .members
            .iter()
            .filter(|m| m.online)
            .filter_map(|m| m.store.get(key).map(|(_, v)| *v))
            .max()
            .unwrap_or(0);
        if current != expected_version {
            return StrongResult::Rejected(RejectReason::VersionConflict);
        }
        let new_version = expected_version + 1;
        let mut online = self.members.iter_mut().filter(|m| m.online).peekable();
        let mut value = Some(new_value);
        while let Some(member) = online.next() {
            // Every member but the last gets a copy; the last takes the value.
            let v = if online.peek().is_some() {
                value.clone().expect("taken only by the last member")
            } else {
                value.take().expect("taken only by the last member")
            };
            // Overwrite in place where the key exists; allocate it only for a
            // member seeing the key for the first time.
            match member.store.get_mut(key) {
                Some(slot) => *slot = (v, new_version),
                None => {
                    member.store.insert(key.to_string(), (v, new_version));
                }
            }
        }
        StrongResult::Committed
    }

    /// Claim a uniqueness `key` for `owner`. Succeeds only if unclaimed; a second
    /// claimant is [`RejectReason::AlreadyClaimed`].
    pub fn claim_unique(&mut self, key: &str, owner: &[u8]) -> StrongResult {
        let (claimed, version) = match self.latest(key) {
            Ok((current, version)) => (current.is_some(), version),
            Err(reason) => return StrongResult::Unavailable(reason),
        };
        if claimed {
            return StrongResult::Rejected(RejectReason::AlreadyClaimed);
        }
        self.cas(key, version, owner.to_vec())
    }

    /// Initialize a non-negative resource `key` with `count` units.
    pub fn init_seats(&mut self, key: &str, count: u64) -> StrongResult {
        let (_, version) = match self.read(key) {
            Ok(read) => read,
            Err(reason) => return StrongResult::Unavailable(reason),
        };
        self.cas(key, version, count.to_le_bytes().to_vec())
    }

    /// Acquire one unit of a non-negative resource. [`RejectReason::Exhausted`] at
    /// zero — never oversells.
    pub fn acquire_seat(&mut self, key: &str) -> StrongResult {
        let (remaining, version) = match self.latest(key) {
            Ok((current, version)) => (decode_count(current), version),
            Err(reason) => return StrongResult::Unavailable(reason),
        };
        if remaining == 0 {
            return StrongResult::Rejected(RejectReason::Exhausted);
        }
        self.cas(key, version, (remaining - 1).to_le_bytes().to_vec())
    }

    /// The units remaining for a resource `key`.
    pub fn seats_remaining(&self, key: &str) -> Result<u64, UnavailableReason> {
        let (current, _) = self.read(key)?;
        Ok(decode_count(current.as_deref()))
    }
}

fn decode_count(bytes: Option<&[u8]>) -> u64 {
    match bytes {
        Some(b) if b.len() == 8 => u64::from_le_bytes(b.try_into().unwrap()),
        _ => 0,
    }
}
