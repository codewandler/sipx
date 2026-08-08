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
//!
//! And it is generic over its **instant**, which is what makes the paragraph above true rather
//! than merely intended. [`tokio::time::Instant`] has only two constructors — `now()`, which reads
//! the machine clock, and `from_std`, which needs a [`std::time::Instant`] that has no zero either
//! — so a discrete-event simulator on virtual time had no instant to hand in and could not build
//! one. The type parameter defaults to [`tokio::time::Instant`], so `TimerQueue<K>` still names
//! exactly what it always named and every existing caller is untouched.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::hash::Hash;
use std::ops::Add;
use std::time::Duration;

use tokio::time::Instant;

#[derive(Debug, PartialEq, Eq)]
struct Entry<K, I> {
    deadline: I,
    generation: u64,
    key: K,
}

// Ordering is by deadline alone, so the bound is on the instant rather than on the key: two
// entries with the same deadline are interchangeable to an earliest-deadline-first queue.
impl<K: Eq, I: Ord> Ord for Entry<K, I> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.deadline.cmp(&other.deadline)
    }
}

impl<K: Eq, I: Ord> PartialOrd for Entry<K, I> {
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
pub struct TimerQueue<K, I = Instant> {
    heap: BinaryHeap<Reverse<Entry<K, I>>>,
    generations: HashMap<K, u64>,
    /// Queue-global identity for the next schedule, so forgetting and reusing a key cannot make a
    /// stale heap entry live again.
    next_generation: u64,
}

// The bounds match `Entry`'s `Ord` impl rather than `new`'s: `BinaryHeap::new` requires its element
// to be `Ord` on our MSRV, and a queue that cannot order its entries has no empty value worth
// naming either. Later toolchains relax the bound, which is why only the MSRV job caught this.
impl<K: Eq, I: Ord> Default for TimerQueue<K, I> {
    fn default() -> Self {
        Self {
            heap: BinaryHeap::new(),
            generations: HashMap::new(),
            next_generation: 0,
        }
    }
}

impl<K: Clone + Eq + Hash, I: Ord + Copy + Add<Duration, Output = I>> TimerQueue<K, I> {
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
    pub fn set(&mut self, key: K, now: I, after: Duration) {
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

    /// Forget one timer key and its generation counter.
    ///
    /// The heap entry, if any, becomes stale and is discarded when it reaches the front. This is
    /// the constant-time termination path for callers that can enumerate their small timer set;
    /// [`Self::forget_matching`] remains the general full-map operation.
    pub fn forget(&mut self, key: &K) {
        self.generations.remove(key);
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
        if self.next_generation == u64::MAX {
            self.compact_generations();
        }
        self.next_generation += 1;
        self.generations.insert(key.clone(), self.next_generation);
        self.next_generation
    }

    /// Discard every stale entry and renumber the live set before the global identity wraps.
    ///
    /// This path requires `u64::MAX` schedules from one queue before it runs. Keeping it complete
    /// avoids either a debug-build overflow panic or a wrapped identity reviving an old entry.
    fn compact_generations(&mut self) {
        let previous = std::mem::take(&mut self.generations);
        let entries = std::mem::take(&mut self.heap);
        self.next_generation = 0;
        for Reverse(mut entry) in entries {
            if previous.get(&entry.key) != Some(&entry.generation) {
                continue;
            }
            self.next_generation += 1;
            entry.generation = self.next_generation;
            self.generations
                .insert(entry.key.clone(), self.next_generation);
            self.heap.push(Reverse(entry));
        }
    }

    /// When the next live timer is due, if any.
    ///
    /// Discards stale entries as it looks, so a queue full of cancelled timers does not keep waking
    /// the loop.
    pub fn next_deadline(&mut self) -> Option<I> {
        loop {
            let Reverse(entry) = self.heap.peek()?;
            if self.is_live(entry) {
                return Some(entry.deadline);
            }
            self.heap.pop();
        }
    }

    /// Take every timer due at or before `now`, earliest first.
    pub fn take_due(&mut self, now: I) -> Vec<K> {
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

    fn is_live(&self, entry: &Entry<K, I>) -> bool {
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
#[cfg_attr(coverage_nightly, coverage(off))]
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

    #[tokio::test(start_paused = true)]
    async fn forgetting_one_timer_discards_its_generation_without_scanning_others() {
        let mut q = Transactions::new();
        let now = Instant::now();
        let forgotten = (key("a"), Timer::A);
        let live = (key("z"), Timer::B);
        q.set(forgotten.clone(), now, Duration::from_millis(100));
        q.set(live.clone(), now, Duration::from_millis(200));

        q.forget(&forgotten);

        assert!(!q.generations.contains_key(&forgotten));
        assert!(q.generations.contains_key(&live));
        assert_eq!(q.take_due(now + Duration::from_millis(300)), vec![live]);
    }

    /// A peer may reuse a transaction key after its previous transaction has terminated. Its new
    /// first timer must not share the old heap entry's generation and make that stale entry live.
    #[tokio::test(start_paused = true)]
    async fn reusing_a_forgotten_key_does_not_revive_its_stale_timer() {
        let mut q = Transactions::new();
        let now = Instant::now();
        let reused = (key("a"), Timer::A);
        q.set(reused.clone(), now, Duration::from_millis(100));
        q.forget(&reused);

        q.set(reused.clone(), now, Duration::from_millis(200));

        assert!(q.take_due(now + Duration::from_millis(100)).is_empty());
        assert_eq!(q.take_due(now + Duration::from_millis(200)), vec![reused]);
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

    /// The story's failing-first test (`X-21`).
    ///
    /// A discrete-event simulator's clock: a counter with a zero, which `tokio::time::Instant` does
    /// not have — its only constructors read the machine clock or take a `std::time::Instant` that
    /// has no zero either. This is the caller the queue was generalised *for*, and until the
    /// instant became a type parameter it was the one caller that could not use it.
    ///
    /// Note there is no `#[tokio::test]` here, and no runtime: the point is a queue that works with
    /// no clock at all, not one that works with a paused clock.
    #[test]
    fn a_virtual_clock_drives_the_queue_with_no_runtime() {
        /// Ticks since the simulation began. Nothing about it can read a clock.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
        struct Virtual(u64);

        impl std::ops::Add<Duration> for Virtual {
            type Output = Self;
            fn add(self, after: Duration) -> Self {
                // A simulation that ran past u64 milliseconds is not a case worth a fallible
                // conversion; saturating keeps the clock monotonic either way.
                Self(
                    self.0
                        .saturating_add(u64::try_from(after.as_millis()).unwrap_or(u64::MAX)),
                )
            }
        }

        let mut q: TimerQueue<&'static str, Virtual> = TimerQueue::new();
        let epoch = Virtual(0);

        q.set("retransmit", epoch, Duration::from_millis(500));
        q.set("give-up", epoch, Duration::from_secs(32));

        assert!(q.take_due(epoch).is_empty(), "nothing is due at the epoch");
        assert_eq!(
            q.next_deadline(),
            Some(Virtual(500)),
            "the queue answers in the caller's own units"
        );
        assert_eq!(q.take_due(Virtual(500)), vec!["retransmit"]);
        assert_eq!(q.take_due(Virtual(31_999)), Vec::<&str>::new());
        assert_eq!(q.take_due(Virtual(32_000)), vec!["give-up"]);
    }

    /// The default type parameter is what keeps this additive: the endpoint's own alias names the
    /// queue with one parameter and still means a `tokio::time::Instant` queue.
    #[tokio::test(start_paused = true)]
    async fn naming_the_queue_without_an_instant_still_means_the_tokio_one() {
        let mut q: TimerQueue<(TransactionKey, Timer)> = TimerQueue::new();
        let now: Instant = Instant::now();
        q.set((key("a"), Timer::A), now, Duration::from_millis(100));
        assert_eq!(q.next_deadline(), Some(now + Duration::from_millis(100)));
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
