//! An earliest-deadline-first timer queue.
//!
//! One queue for a whole driver, not a task per timer. A busy proxy holds tens of thousands of live
//! timers; spawning a task for each is how a stack acquires a scheduling problem nobody can profile.
//!
//! **The queue does not read the clock.** `now` is an argument to [`TimerQueue::set`] and to
//! [`TimerQueue::take_due`], so a driver on virtual time — or a test asserting *when* a
//! retransmission was scheduled rather than sleeping until it happens — uses the same queue the
//! endpoint does. A queue that called `Instant::now()` internally would be unusable by either,
//! which is what this one used to be.
//!
//! It is generic over its key for the same reason: the endpoint keys on
//! `(TransactionKey, Timer)`, and nothing about earliest-deadline-first scheduling cares.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::hash::Hash;

use tokio::time::Instant;

#[derive(Debug, PartialEq, Eq)]
struct Entry<K> {
    deadline: Instant,
    generation: u64,
    key: K,
}

impl<K: Eq> Ord for Entry<K> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.deadline.cmp(&other.deadline)
    }
}

impl<K: Eq> PartialOrd for Entry<K> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Pending timers, earliest first.
///
/// Cancellation does not remove from the middle of the heap. Each key carries a generation counter;
/// setting or clearing bumps it, and an entry whose generation is stale is discarded when it
/// surfaces. Cancellation is common — every response cancels something — so making it O(1) and
/// paying at pop time is the right trade.
#[derive(Debug)]
pub struct TimerQueue<K> {
    heap: BinaryHeap<Reverse<Entry<K>>>,
    generations: HashMap<K, u64>,
}

impl<K> Default for TimerQueue<K> {
    fn default() -> Self {
        Self {
            heap: BinaryHeap::new(),
            generations: HashMap::new(),
        }
    }
}

impl<K: Clone + Eq + Hash> TimerQueue<K> {
    /// An empty queue.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many entries are held, including stale ones not yet discarded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.heap.len()
    }

    /// Whether nothing is scheduled.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    /// Schedule a timer for `after` from `now`, replacing any previous instance of the same key.
    ///
    /// `now` is the caller's, not this queue's. That is the whole difference between a queue a
    /// tokio driver can use and one that any driver can: a caller on virtual time hands its own
    /// clock in, and nothing here has an opinion about what an instant means.
    pub fn set(&mut self, key: K, now: Instant, after: std::time::Duration) {
        let generation = self.bump(&key);
        self.heap.push(Reverse(Entry {
            deadline: now + after,
            generation,
            key,
        }));
    }

    /// Cancel a timer.
    pub fn clear(&mut self, key: &K) {
        self.bump(key);
    }

    /// Cancel every timer whose key matches.
    ///
    /// The general form of "cancel everything belonging to this transaction", which is what
    /// termination means — expressed as a predicate because the queue does not know what part of a
    /// key identifies a transaction.
    pub fn clear_matching(&mut self, matches: impl Fn(&K) -> bool) {
        let keys: Vec<K> = self
            .generations
            .keys()
            .filter(|key| matches(key))
            .cloned()
            .collect();
        for key in keys {
            self.bump(&key);
        }
    }

    fn bump(&mut self, key: &K) -> u64 {
        let slot = self.generations.entry(key.clone()).or_insert(0);
        *slot += 1;
        *slot
    }

    /// When the next live timer is due, if any.
    ///
    /// Discards stale entries as it looks, so a queue full of cancelled timers does not keep waking
    /// the loop.
    pub fn next_deadline(&mut self) -> Option<Instant> {
        loop {
            let Reverse(entry) = self.heap.peek()?;
            if self.is_live(entry) {
                return Some(entry.deadline);
            }
            self.heap.pop();
        }
    }

    /// Take every timer due at or before `now`, earliest first.
    pub fn take_due(&mut self, now: Instant) -> Vec<K> {
        let mut fired = Vec::new();
        while let Some(Reverse(entry)) = self.heap.peek() {
            if entry.deadline > now {
                break;
            }
            let Some(Reverse(entry)) = self.heap.pop() else {
                break;
            };
            if !self.is_live(&entry) {
                continue;
            }
            // Firing consumes the schedule: a later entry for the same key, set by whatever this
            // fire produces, gets a fresh generation.
            self.bump(&entry.key);
            fired.push(entry.key);
        }
        fired
    }

    fn is_live(&self, entry: &Entry<K>) -> bool {
        self.generations
            .get(&entry.key)
            .is_some_and(|&generation| generation == entry.generation)
    }

    /// Forget every key that matches, generation counters and all.
    pub fn forget_matching(&mut self, matches: impl Fn(&K) -> bool) {
        self.generations.retain(|key, _| !matches(key));
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use sipx_sip::transaction::{Timer, TransactionKey};
    use std::time::Duration;

    fn key(branch: &str) -> TransactionKey {
        TransactionKey::Rfc3261 {
            branch: branch.as_bytes().to_vec(),
            sent_by: b"h.example.com".to_vec(),
            method: b"INVITE".to_vec(),
        }
    }

    /// The endpoint's key type, which is what the generic parameter exists to accommodate.
    type Transactions = TimerQueue<(TransactionKey, Timer)>;

    #[tokio::test(start_paused = true)]
    async fn timers_fire_in_deadline_order() {
        let mut q = Transactions::new();
        let now = Instant::now();
        q.set((key("a"), Timer::A), now, Duration::from_millis(500));
        q.set((key("b"), Timer::B), now, Duration::from_millis(100));
        q.set((key("c"), Timer::E), now, Duration::from_millis(300));

        let fired = q.take_due(now + Duration::from_millis(600));
        let order: Vec<Timer> = fired.iter().map(|(_, timer)| *timer).collect();
        assert_eq!(order, vec![Timer::B, Timer::E, Timer::A]);
    }

    /// The queue never reads the clock, so a caller can schedule and fire without any time
    /// passing at all — which is what a virtual-time driver does and what a test that would
    /// otherwise sleep wants.
    #[tokio::test]
    async fn scheduling_and_firing_need_no_real_time_to_pass() {
        let mut q = Transactions::new();
        let epoch = Instant::now();
        q.set((key("a"), Timer::A), epoch, Duration::from_secs(3600));

        assert!(q.take_due(epoch).is_empty(), "not due yet");
        assert_eq!(
            q.take_due(epoch + Duration::from_secs(3600)).len(),
            1,
            "an hour later, without an hour passing"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_cleared_timer_does_not_fire() {
        let mut q = Transactions::new();
        let now = Instant::now();
        q.set((key("a"), Timer::A), now, Duration::from_millis(100));
        q.clear(&(key("a"), Timer::A));

        assert!(q.take_due(now + Duration::from_millis(200)).is_empty());
    }

    /// Re-setting a timer replaces it rather than adding a second one — the retransmission case,
    /// which happens on every fire.
    #[tokio::test(start_paused = true)]
    async fn resetting_a_timer_replaces_it() {
        let mut q = Transactions::new();
        let now = Instant::now();
        q.set((key("a"), Timer::A), now, Duration::from_millis(100));
        q.set((key("a"), Timer::A), now, Duration::from_millis(500));

        assert!(
            q.take_due(now + Duration::from_millis(200)).is_empty(),
            "the first schedule must not survive"
        );
        assert_eq!(q.take_due(now + Duration::from_millis(600)).len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn clearing_a_transaction_clears_all_of_its_timers() {
        let mut q = Transactions::new();
        let now = Instant::now();
        q.set((key("a"), Timer::A), now, Duration::from_millis(100));
        q.set((key("a"), Timer::B), now, Duration::from_millis(200));
        q.set((key("z"), Timer::A), now, Duration::from_millis(100));
        q.clear_matching(|(k, _)| k == &key("a"));

        let fired = q.take_due(now + Duration::from_millis(300));
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].0, key("z"));
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_entries_do_not_keep_waking_the_loop() {
        let mut q = Transactions::new();
        let now = Instant::now();
        for i in 0..100 {
            q.set(
                (key(&format!("k{i}")), Timer::A),
                now,
                Duration::from_millis(10),
            );
            q.clear(&(key(&format!("k{i}")), Timer::A));
        }
        q.set((key("live"), Timer::A), now, Duration::from_secs(60));

        // The next deadline is the live one, not any of the hundred dead entries.
        let deadline = q.next_deadline().expect("a deadline");
        assert!(deadline >= now + Duration::from_secs(59));
        assert_eq!(q.len(), 1, "stale entries are discarded while looking");
    }

    /// The key is opaque to the queue: anything hashable schedules.
    #[tokio::test(start_paused = true)]
    async fn the_queue_schedules_any_key_at_all() {
        let mut q: TimerQueue<&'static str> = TimerQueue::new();
        let now = Instant::now();
        q.set("refresh", now, Duration::from_millis(50));
        q.set("keepalive", now, Duration::from_millis(10));
        assert_eq!(
            q.take_due(now + Duration::from_millis(100)),
            vec!["keepalive", "refresh"]
        );
    }
}
