//! A byte link between two stacks in one process, with faults you can turn on.
//!
//! The loopback transport this crate's documentation has promised since it was written. Two full
//! stacks talk through it with no sockets, no ports and no sleeping — which is what makes the
//! behaviour the transaction machines exist for testable at all. A retransmission after a lost
//! datagram takes 500 milliseconds of real time over a real socket and none at all over this.
//!
//! **The link does not read the clock.** `now` is an argument, exactly as it is for
//! [`sipx_transport::timers::TimerQueue`], so a test drives both from one virtual clock of its own
//! and a lost packet costs no wall time.
//!
//! **Faults are seeded.** The same seed replays the same trace, so a failure found by fuzzing loss
//! rates is a failure you can re-run. A link whose faults came from a thread RNG would produce bug
//! reports nobody could reproduce.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::time::Duration;

use bytes::Bytes;
use tokio::time::Instant;

/// Which end of the link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    /// The end that opened the conversation.
    Left,
    /// The other one.
    Right,
}

impl Side {
    /// The end a datagram sent from here arrives at.
    #[must_use]
    pub fn peer(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }
}

/// What the link does to the traffic crossing it.
///
/// All zero by default: a link with no faults is a wire, and a test that wants one should not have
/// to say so.
#[derive(Debug, Clone, Copy, Default)]
pub struct Faults {
    /// Probability in `0.0..=1.0` that a datagram is dropped outright.
    pub loss: f64,
    /// Probability that a datagram is delivered twice.
    ///
    /// Worth having as its own knob rather than folding into loss: a duplicate is what makes a
    /// receiver's idempotence testable, and RFC 3261 §17 is largely about absorbing them.
    pub duplicate: f64,
    /// The base one-way delay.
    pub latency: Duration,
    /// How much the delay varies, uniformly, either side of `latency`.
    ///
    /// This is also where **reordering** comes from, and deliberately so: packets do not overtake
    /// each other because a network chose to reorder them, they overtake because one took longer
    /// than another. A separate "reorder" probability would model the symptom instead of the cause,
    /// and would let a test see an ordering no real path could produce.
    pub jitter: Duration,
}

impl Faults {
    /// A link that loses this fraction of datagrams and nothing else.
    #[must_use]
    pub fn losing(loss: f64) -> Self {
        Self {
            loss,
            ..Self::default()
        }
    }

    /// A link that drops nothing but takes this long.
    #[must_use]
    pub fn delayed(latency: Duration) -> Self {
        Self {
            latency,
            ..Self::default()
        }
    }
}

/// A datagram that has arrived.
#[derive(Debug, Clone)]
pub struct Delivery {
    /// Which end it arrived at.
    pub to: Side,
    /// The bytes, unaltered — the link corrupts nothing, because a corrupted SIP message is the
    /// parser's business and there is a fuzzer for that.
    pub bytes: Bytes,
}

#[derive(Debug, PartialEq, Eq)]
struct Scheduled {
    at: Instant,
    /// Breaks ties in arrival order so two datagrams scheduled for the same instant deliver in the
    /// order they were sent. Without it the heap's tie-break is arbitrary and the same seed
    /// produces different traces.
    sequence: u64,
    to: Side,
    bytes: Bytes,
}

impl Ord for Scheduled {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.at
            .cmp(&other.at)
            .then_with(|| self.sequence.cmp(&other.sequence))
    }
}

impl PartialOrd for Scheduled {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// An in-process link between two stacks.
#[derive(Debug)]
pub struct Link {
    faults: Faults,
    state: u64,
    sequence: u64,
    in_flight: BinaryHeap<Reverse<Scheduled>>,
    /// Datagrams the link dropped, for a test that wants to assert it dropped one.
    dropped: u64,
}

impl Link {
    /// A link with these faults, replaying the same trace for the same seed.
    #[must_use]
    pub fn new(seed: u64, faults: Faults) -> Self {
        Self {
            faults,
            // Any non-zero state; splitmix64 is uniform from anywhere.
            state: seed.wrapping_add(0x9E37_79B9_7F4A_7C15),
            sequence: 0,
            in_flight: BinaryHeap::new(),
            dropped: 0,
        }
    }

    /// A link that loses, duplicates and delays nothing.
    #[must_use]
    pub fn perfect() -> Self {
        Self::new(0, Faults::default())
    }

    /// How many datagrams the link has dropped.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// How many datagrams are in flight.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.in_flight.len()
    }

    /// Hand a datagram to the link, to arrive at some point at or after `now`.
    pub fn send(&mut self, from: Side, bytes: Bytes, now: Instant) {
        if self.chance() < self.faults.loss {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        self.schedule(from.peer(), bytes.clone(), now);
        if self.chance() < self.faults.duplicate {
            // A second copy, drawn its own delay — so a duplicate can arrive before *or* after the
            // original, which is what a duplicating path actually does.
            self.schedule(from.peer(), bytes, now);
        }
    }

    fn schedule(&mut self, to: Side, bytes: Bytes, now: Instant) {
        let delay = self.delay();
        self.sequence = self.sequence.wrapping_add(1);
        self.in_flight.push(Reverse(Scheduled {
            at: now + delay,
            sequence: self.sequence,
            to,
            bytes,
        }));
    }

    /// Everything that has arrived at or before `now`, in arrival order.
    pub fn take_due(&mut self, now: Instant) -> Vec<Delivery> {
        let mut arrived = Vec::new();
        while let Some(Reverse(next)) = self.in_flight.peek() {
            if next.at > now {
                break;
            }
            let Some(Reverse(scheduled)) = self.in_flight.pop() else {
                break;
            };
            arrived.push(Delivery {
                to: scheduled.to,
                bytes: scheduled.bytes,
            });
        }
        arrived
    }

    /// When the next datagram arrives, if any is in flight.
    #[must_use]
    pub fn next_arrival(&self) -> Option<Instant> {
        self.in_flight.peek().map(|Reverse(next)| next.at)
    }

    /// The one-way delay for the next datagram.
    fn delay(&mut self) -> Duration {
        if self.faults.jitter.is_zero() {
            return self.faults.latency;
        }
        let spread = self.faults.jitter.as_nanos().min(u128::from(u64::MAX));
        #[expect(
            clippy::cast_possible_truncation,
            reason = "clamped to u64::MAX on the line above"
        )]
        let spread = spread as u64;
        // Uniform in `latency - jitter ..= latency + jitter`, saturating at zero: a delay cannot be
        // negative, and clamping is more honest than wrapping into an enormous one.
        let offset = self.next_u64() % (spread.saturating_mul(2).saturating_add(1));
        let base = Duration::from_nanos(offset);
        (self.faults.latency + base).saturating_sub(self.faults.jitter)
    }

    /// A draw in `0.0..1.0`.
    fn chance(&mut self) -> f64 {
        // 53 bits, which is every value an `f64` can represent exactly in this range.
        #[expect(
            clippy::cast_precision_loss,
            reason = "53 bits is exactly what an f64 represents; no precision is lost"
        )]
        let value = (self.next_u64() >> 11) as f64;
        // `2^53` written as a literal rather than cast from `u64`, so the divisor is exact by
        // construction instead of exact by argument.
        value / 9_007_199_254_740_992.0_f64
    }

    /// splitmix64 — small, seedable, and good enough for choosing which packets to drop.
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
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

    fn datagram(text: &'static str) -> Bytes {
        Bytes::from_static(text.as_bytes())
    }

    #[tokio::test(start_paused = true)]
    async fn a_perfect_link_delivers_everything_immediately() {
        let mut link = Link::perfect();
        let now = Instant::now();
        link.send(Side::Left, datagram("one"), now);
        link.send(Side::Right, datagram("two"), now);

        let arrived = link.take_due(now);
        assert_eq!(arrived.len(), 2);
        assert_eq!(arrived[0].to, Side::Right, "left's datagram goes right");
        assert_eq!(arrived[1].to, Side::Left);
        assert_eq!(link.dropped(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn a_link_that_loses_everything_delivers_nothing() {
        let mut link = Link::new(1, Faults::losing(1.0));
        let now = Instant::now();
        for _ in 0..10u32 {
            link.send(Side::Left, datagram("x"), now);
        }
        assert!(link.take_due(now).is_empty());
        assert_eq!(link.dropped(), 10);
    }

    #[tokio::test(start_paused = true)]
    async fn a_delayed_datagram_does_not_arrive_early() {
        let mut link = Link::new(1, Faults::delayed(Duration::from_millis(50)));
        let now = Instant::now();
        link.send(Side::Left, datagram("x"), now);

        assert!(link.take_due(now).is_empty(), "not yet");
        assert_eq!(link.next_arrival(), Some(now + Duration::from_millis(50)));
        assert_eq!(link.take_due(now + Duration::from_millis(50)).len(), 1);
    }

    /// The same seed replays the same trace. Without this a failure found at a given loss rate is
    /// not a failure anybody can re-run.
    #[tokio::test(start_paused = true)]
    async fn one_seed_replays_one_trace() {
        let trace = |seed: u64| {
            let mut link = Link::new(seed, Faults::losing(0.5));
            let now = Instant::now();
            let mut delivered = Vec::new();
            for index in 0..40u32 {
                link.send(Side::Left, Bytes::from(index.to_string()), now);
            }
            for delivery in link.take_due(now) {
                delivered.push(String::from_utf8_lossy(&delivery.bytes).into_owned());
            }
            delivered
        };
        assert_eq!(trace(7), trace(7), "one seed, one trace");
        assert_ne!(
            trace(7),
            trace(8),
            "and different seeds explore different traces, or fuzzing the seed does nothing"
        );
    }

    /// Loss is roughly the rate asked for. A link whose knob does not move is a link that tests
    /// nothing, and a rate that is silently zero would make every fault test pass.
    #[tokio::test(start_paused = true)]
    async fn the_loss_rate_is_about_what_was_asked_for() {
        let mut link = Link::new(42, Faults::losing(0.25));
        let now = Instant::now();
        let total = 4000u32;
        for _ in 0..total {
            link.send(Side::Left, datagram("x"), now);
        }
        let lost = link.dropped();
        assert!(
            (800..1200).contains(&lost),
            "a quarter of 4000 should be near 1000, got {lost}"
        );
    }

    /// Jitter reorders, because one datagram took longer than another — not because the link
    /// decided to shuffle them.
    #[tokio::test(start_paused = true)]
    async fn jitter_lets_a_later_datagram_arrive_first() {
        let mut link = Link::new(
            3,
            Faults {
                latency: Duration::from_millis(50),
                jitter: Duration::from_millis(40),
                ..Faults::default()
            },
        );
        let now = Instant::now();
        for index in 0..20u32 {
            link.send(Side::Left, Bytes::from(index.to_string()), now);
        }
        let order: Vec<String> = link
            .take_due(now + Duration::from_millis(200))
            .into_iter()
            .map(|delivery| String::from_utf8_lossy(&delivery.bytes).into_owned())
            .collect();
        let sent: Vec<String> = (0..20u32).map(|index| index.to_string()).collect();
        assert_eq!(order.len(), sent.len(), "nothing is lost, only reordered");
        assert_ne!(order, sent, "with 40ms of jitter something must overtake");
    }

    #[tokio::test(start_paused = true)]
    async fn duplication_delivers_a_datagram_twice() {
        let mut link = Link::new(
            5,
            Faults {
                duplicate: 1.0,
                ..Faults::default()
            },
        );
        let now = Instant::now();
        link.send(Side::Left, datagram("x"), now);
        assert_eq!(
            link.take_due(now).len(),
            2,
            "a duplicating link delivers the same datagram twice"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn datagrams_scheduled_together_arrive_in_the_order_they_were_sent() {
        // No jitter, so every delay is equal and only the tie-break decides. Without a stable one
        // the heap's order is arbitrary and a seeded trace is not reproducible.
        let mut link = Link::new(1, Faults::delayed(Duration::from_millis(10)));
        let now = Instant::now();
        for index in 0..8u32 {
            link.send(Side::Left, Bytes::from(index.to_string()), now);
        }
        let order: Vec<String> = link
            .take_due(now + Duration::from_millis(10))
            .into_iter()
            .map(|delivery| String::from_utf8_lossy(&delivery.bytes).into_owned())
            .collect();
        assert_eq!(order, (0..8u32).map(|i| i.to_string()).collect::<Vec<_>>());
    }
}
