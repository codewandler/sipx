//! The endpoint's timer queue.
//!
//! One earliest-deadline-first queue for the whole endpoint, not a task per timer. A busy
//! proxy holds tens of thousands of live timers; spawning a task for each is how a stack
//! acquires a scheduling problem nobody can profile.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use sipx_sip::transaction::{Timer, TransactionKey};
use tokio::time::Instant;

/// A timer that has come due.
#[derive(Debug, Clone)]
pub struct Fired {
    /// The transaction it belongs to.
    pub key: TransactionKey,
    /// Which timer.
    pub timer: Timer,
}

#[derive(Debug, PartialEq, Eq)]
struct Entry {
    deadline: Instant,
    generation: u64,
    key: TransactionKey,
    timer: Timer,
}

impl Ord for Entry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.deadline.cmp(&other.deadline)
    }
}

impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Pending timers, earliest first.
///
/// Cancellation does not remove from the middle of the heap. Each `(transaction, timer)` pair
/// carries a generation counter; setting or clearing bumps it, and an entry whose generation
/// is stale is discarded when it surfaces. Cancellation is common — every response cancels
/// something — so making it O(1) and paying at pop time is the right trade.
#[derive(Debug, Default)]
pub struct TimerQueue {
    heap: BinaryHeap<Reverse<Entry>>,
    generations: HashMap<(TransactionKey, Timer), u64>,
}

impl TimerQueue {
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

    /// Schedule a timer, replacing any previous instance of the same one.
    pub fn set(&mut self, key: TransactionKey, timer: Timer, after: std::time::Duration) {
        let generation = self.bump(&key, timer);
        self.heap.push(Reverse(Entry {
            deadline: Instant::now() + after,
            generation,
            key,
            timer,
        }));
    }

    /// Cancel a timer.
    pub fn clear(&mut self, key: &TransactionKey, timer: Timer) {
        self.bump(key, timer);
    }

    /// Cancel every timer of a transaction, which is what termination means.
    pub fn clear_all(&mut self, key: &TransactionKey) {
        let timers: Vec<Timer> = self
            .generations
            .keys()
            .filter(|(k, _)| k == key)
            .map(|(_, t)| *t)
            .collect();
        for timer in timers {
            self.bump(key, timer);
        }
    }

    fn bump(&mut self, key: &TransactionKey, timer: Timer) -> u64 {
        let slot = self.generations.entry((key.clone(), timer)).or_insert(0);
        *slot += 1;
        *slot
    }

    /// When the next live timer is due, if any.
    ///
    /// Discards stale entries as it looks, so a queue full of cancelled timers does not keep
    /// waking the loop.
    pub fn next_deadline(&mut self) -> Option<Instant> {
        loop {
            let Reverse(entry) = self.heap.peek()?;
            if self.is_live(entry) {
                return Some(entry.deadline);
            }
            self.heap.pop();
        }
    }

    /// Take every timer due at or before `now`.
    pub fn take_due(&mut self, now: Instant) -> Vec<Fired> {
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
            // Firing consumes the schedule: a later entry for the same timer, set by whatever
            // this fire produces, gets a fresh generation.
            self.bump(&entry.key, entry.timer);
            fired.push(Fired {
                key: entry.key,
                timer: entry.timer,
            });
        }
        fired
    }

    fn is_live(&self, entry: &Entry) -> bool {
        self.generations
            .get(&(entry.key.clone(), entry.timer))
            .is_some_and(|&g| g == entry.generation)
    }

    /// Forget a transaction entirely.
    pub fn forget(&mut self, key: &TransactionKey) {
        self.generations.retain(|(k, _), _| k != key);
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
    use std::time::Duration;

    fn key(branch: &str) -> TransactionKey {
        TransactionKey::Rfc3261 {
            branch: branch.as_bytes().to_vec(),
            sent_by: b"h.example.com".to_vec(),
            method: b"INVITE".to_vec(),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn timers_fire_in_deadline_order() {
        let mut q = TimerQueue::new();
        q.set(key("a"), Timer::A, Duration::from_millis(500));
        q.set(key("b"), Timer::B, Duration::from_millis(100));
        q.set(key("c"), Timer::E, Duration::from_millis(300));

        tokio::time::advance(Duration::from_millis(600)).await;
        let fired = q.take_due(Instant::now());
        let order: Vec<Timer> = fired.iter().map(|f| f.timer).collect();
        assert_eq!(order, vec![Timer::B, Timer::E, Timer::A]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_cleared_timer_does_not_fire() {
        let mut q = TimerQueue::new();
        q.set(key("a"), Timer::A, Duration::from_millis(100));
        q.clear(&key("a"), Timer::A);

        tokio::time::advance(Duration::from_millis(200)).await;
        assert!(q.take_due(Instant::now()).is_empty());
    }

    /// Re-setting a timer replaces it rather than adding a second one — the retransmission
    /// case, which happens on every fire.
    #[tokio::test(start_paused = true)]
    async fn resetting_a_timer_replaces_it() {
        let mut q = TimerQueue::new();
        q.set(key("a"), Timer::A, Duration::from_millis(100));
        q.set(key("a"), Timer::A, Duration::from_millis(500));

        tokio::time::advance(Duration::from_millis(200)).await;
        assert!(
            q.take_due(Instant::now()).is_empty(),
            "the first schedule must not survive"
        );

        tokio::time::advance(Duration::from_millis(400)).await;
        assert_eq!(q.take_due(Instant::now()).len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn clearing_a_transaction_clears_all_of_its_timers() {
        let mut q = TimerQueue::new();
        q.set(key("a"), Timer::A, Duration::from_millis(100));
        q.set(key("a"), Timer::B, Duration::from_millis(200));
        q.set(key("z"), Timer::A, Duration::from_millis(100));
        q.clear_all(&key("a"));

        tokio::time::advance(Duration::from_millis(300)).await;
        let fired = q.take_due(Instant::now());
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].key, key("z"));
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_entries_do_not_keep_waking_the_loop() {
        let mut q = TimerQueue::new();
        for i in 0..100 {
            q.set(key(&format!("k{i}")), Timer::A, Duration::from_millis(10));
            q.clear(&key(&format!("k{i}")), Timer::A);
        }
        q.set(key("live"), Timer::A, Duration::from_secs(60));

        // The next deadline is the live one, not any of the hundred dead entries.
        let deadline = q.next_deadline().expect("a deadline");
        assert!(deadline >= Instant::now() + Duration::from_secs(59));
        assert_eq!(q.len(), 1, "stale entries are discarded while looking");
    }
}
