//! The virtual clock + discrete-event queue.
//!
//! Time only advances when an event is popped, so a simulated week runs in
//! milliseconds. Events are ordered by `(at, seq)` — a monotonic `seq` breaks ties,
//! giving a *total* order that never depends on wall-clock or map iteration, so the
//! whole run is reproducible.

use std::collections::BinaryHeap;
use std::cmp::Ordering;

struct Scheduled<E> {
    at: u64,
    seq: u64,
    event: E,
}

impl<E> PartialEq for Scheduled<E> {
    fn eq(&self, other: &Self) -> bool {
        self.at == other.at && self.seq == other.seq
    }
}
impl<E> Eq for Scheduled<E> {}

impl<E> Ord for Scheduled<E> {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reversed so the max-heap yields the *earliest* event first.
        other
            .at
            .cmp(&self.at)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}
impl<E> PartialOrd for Scheduled<E> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A virtual-time event scheduler.
pub struct Scheduler<E> {
    now: u64,
    seq: u64,
    heap: BinaryHeap<Scheduled<E>>,
}

impl<E> Default for Scheduler<E> {
    fn default() -> Self {
        Self {
            now: 0,
            seq: 0,
            heap: BinaryHeap::new(),
        }
    }
}

impl<E> Scheduler<E> {
    pub fn new() -> Self {
        Self::default()
    }

    /// The current virtual time.
    pub fn now(&self) -> u64 {
        self.now
    }

    /// Schedule `event` to fire `delay` ticks from now.
    pub fn schedule(&mut self, delay: u64, event: E) {
        let at = self.now + delay;
        self.schedule_at(at, event);
    }

    /// Schedule `event` to fire at absolute time `at` (clamped to not precede now).
    pub fn schedule_at(&mut self, at: u64, event: E) {
        let at = at.max(self.now);
        self.heap.push(Scheduled {
            at,
            seq: self.seq,
            event,
        });
        self.seq += 1;
    }

    /// Pop the next event, advancing the clock to its time.
    pub fn pop(&mut self) -> Option<E> {
        self.heap.pop().map(|s| {
            self.now = s.at;
            s.event
        })
    }
}
