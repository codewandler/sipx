//! What the buffer costs, measured the way the media session actually drains it (`M-45`).
//!
//! [`jitter_traces`](jitter_traces.rs) compares the two policies against each other on a fixed
//! playout clock. This file asks a different question, and it is the one two field reports about
//! "seconds of delay" made worth asking: **how long does a packet sit in the buffer before it is
//! played, and can that grow?**
//!
//! The consumer here is therefore not a metronome. It is the shape `sipx-media`'s receive loop
//! has: push on arrival, then pop until the buffer refuses, and drain what is held after a
//! silence. That distinction is the whole point — a metronome hides growth by measuring the
//! clock instead of the buffer, and on the arrival-driven drain the hold time *is* the latency
//! the buffer is charging.
//!
//! Time is a `u64` of milliseconds stepped by the loop. There is no sleep here and there must
//! never be one (`scripts/check-fixed-sleep.py`); a trace that took real time to run could not
//! afford the 30 seconds of audio the growth assertions need.
//!
//! ## The measured baseline, for the next person to change this
//!
//! 1500 packets of G.711 at 20 ms, `adaptive(3, 12)` unless noted, hold in milliseconds:
//!
//! | trace | hold max | hold mean | final depth | peak depth | late | lost | dup |
//! |---|---|---|---|---|---|---|---|
//! | constant 5 ms delay | 100 | 40.1 | 3 | 3 | 0 | 0 | 0 |
//! | jitter, 5–65 ms | 114 | 40.1 | 3 | 3 | 0 | 0 | 0 |
//! | 300 ms spike every third packet | 515 | 200.8 | 11 | 11 | 13 | 12 | 0 |
//! | 9 % loss | 100 | 44.1 | 3 | 3 | 0 | 136 | 0 |
//! | every other pair swapped | 85 | 40.1 | 3 | 3 | 0 | 0 | 0 |
//! | 20 % duplicated | 100 | 39.5 | 3 | 3 | 0 | 0 | 300 |
//! | one packet 3 s late | 100 | 60.9 | 3 | 6 | 1 | 1 | 0 |
//! | 1 s stall, then 50 at once | 140 | 85.5 | 3 | 8 | 0 | 0 | 0 |
//!
//! The conclusion that shaped `M-45`: **the buffer is not where seconds come from.** The worst
//! hold in any of those traces is half a second, on a network delivering a third of its packets
//! 300 ms late, and the depth came back down to 3 after both the straggler and the stall. What
//! the buffer did *not* do was conceal the gaps it counted — that was the defect, and it is
//! fixed in `sipx-media`'s receive loop rather than here.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss
)]

use bytes::Bytes;
use sipx_rtp::JitterBuffer;
use sipx_rtp::packet::Packet;

/// G.711: 8000 timestamp units per second, 160 per 20 ms packet.
const CLOCK: u32 = 8000;
const INTERVAL: u32 = 160;
const PACKET_MS: u64 = 20;
/// `sipx-media`'s `flush_after`: `max(4 · packet_duration, 60 ms)` of quiet releases what is held.
const FLUSH_AFTER_MS: u64 = 80;

/// One packet turning up on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Arrival {
    at_ms: u64,
    sequence: u16,
}

/// What one trace cost.
#[derive(Debug, Default)]
struct Playout {
    /// Every packet played, in the order it was played, with the instant it was played at.
    played: Vec<(u16, u64)>,
    /// The longest any packet waited between arriving and being played. This is the latency the
    /// buffer is charging, and the number the field reports were about.
    hold_max_ms: u64,
    hold_total_ms: u64,
    /// The deepest the buffer ever got, and where it ended. Both, because "it came back down"
    /// cannot be told from "it never went up" with only one of them.
    depth_peak: usize,
    depth_final: usize,
    late: u64,
    lost: u64,
    duplicates: u64,
}

impl Playout {
    fn hold_mean_ms(&self) -> f64 {
        if self.played.is_empty() {
            0.0
        } else {
            self.hold_total_ms as f64 / self.played.len() as f64
        }
    }

    fn sequences(&self) -> Vec<u16> {
        self.played.iter().map(|(sequence, _)| *sequence).collect()
    }

    /// The numbers, without the play-out list. A failure message carrying 1500 tuples is one
    /// nobody reads.
    fn summary(&self) -> String {
        format!(
            "played={} hold_max={}ms hold_mean={:.1}ms depth={} peak={} late={} lost={} dup={}",
            self.played.len(),
            self.hold_max_ms,
            self.hold_mean_ms(),
            self.depth_final,
            self.depth_peak,
            self.late,
            self.lost,
            self.duplicates,
        )
    }

    /// The hold time of everything played after `from_ms`.
    ///
    /// Growth is a question about the *end* of a long trace, not its average: a buffer that
    /// settles at 40 ms and one that is at four seconds and climbing have similar means over the
    /// first minute.
    fn hold_max_after(&self, from_ms: u64, arrivals: &[Arrival]) -> u64 {
        self.played
            .iter()
            .filter(|(_, at)| *at >= from_ms)
            .filter_map(|(sequence, at)| {
                arrivals
                    .iter()
                    .find(|a| a.sequence == *sequence)
                    .map(|a| at.saturating_sub(a.at_ms))
            })
            .max()
            .unwrap_or(0)
    }
}

fn packet(sequence: u16) -> Packet {
    Packet::new(
        0,
        sequence,
        u32::from(sequence).wrapping_mul(INTERVAL),
        1,
        Bytes::from(vec![0u8; INTERVAL as usize]),
    )
}

/// Score one release: how long it waited, and where it landed in the played order.
fn record(
    out: &mut Playout,
    arrived: &std::collections::HashMap<u16, u64>,
    released: &Packet,
    now: u64,
) {
    let held = now.saturating_sub(arrived.get(&released.sequence).copied().unwrap_or(now));
    out.hold_max_ms = out.hold_max_ms.max(held);
    out.hold_total_ms += held;
    out.played.push((released.sequence, now));
}

/// Drive a trace through a buffer the way `sipx-media`'s receive loop drives one.
///
/// Push on arrival, pop until it refuses, and drain after `FLUSH_AFTER_MS` of quiet. Nothing
/// here reads a real clock.
fn play(mut buffer: JitterBuffer, arrivals: &[Arrival], horizon_ms: u64) -> Playout {
    let mut ordered = arrivals.to_vec();
    ordered.sort_by_key(|a| a.at_ms);

    let mut out = Playout {
        depth_peak: buffer.depth(),
        ..Playout::default()
    };
    let mut arrived: std::collections::HashMap<u16, u64> = std::collections::HashMap::new();
    let mut next = 0usize;
    let mut last_datagram_ms = 0u64;

    for now in 0..horizon_ms {
        let mut saw_datagram = false;
        while next < ordered.len() && ordered[next].at_ms == now {
            let sequence = ordered[next].sequence;
            arrived.entry(sequence).or_insert(now);
            let arrival = (now * u64::from(CLOCK) / 1000) as u32;
            let _kept = buffer.push_at(packet(sequence), arrival);
            out.depth_peak = out.depth_peak.max(buffer.depth());
            next += 1;
            saw_datagram = true;

            while let Some(released) = buffer.pop() {
                record(&mut out, &arrived, &released, now);
            }
        }

        if saw_datagram {
            last_datagram_ms = now;
        } else if now.saturating_sub(last_datagram_ms) >= FLUSH_AFTER_MS && !buffer.is_empty() {
            for released in buffer.drain() {
                record(&mut out, &arrived, &released, now);
            }
            last_datagram_ms = now;
        }
    }

    out.late = buffer.late();
    out.lost = buffer.lost();
    out.duplicates = buffer.duplicates();
    out.depth_final = buffer.depth();
    out
}

/// A packet per `PACKET_MS`, each `delays_ms[i]` late.
fn trace(delays_ms: &[u64]) -> Vec<Arrival> {
    delays_ms
        .iter()
        .enumerate()
        .map(|(i, delay)| Arrival {
            at_ms: i as u64 * PACKET_MS + delay,
            sequence: i as u16,
        })
        .collect()
}

/// Deterministic, so a failure is reproducible; xorshift because the distribution does not
/// matter here and a seeded generator that anyone can re-run does.
fn jitter(count: usize, low: u64, high: u64) -> Vec<u64> {
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    (0..count)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            low + state % (high - low + 1)
        })
        .collect()
}

/// The play-out order under jitter and reordering, which is what the buffer is for.
///
/// Asserted as the whole sequence rather than as "it came out sorted", because a buffer that
/// dropped every third packet would also come out sorted.
#[test]
fn jitter_and_reordering_are_played_back_in_the_order_they_were_sent() {
    // Every other packet held back 45 ms, which lands it behind its successor on the wire.
    let delays: Vec<u64> = (0..400).map(|i| if i % 2 == 0 { 45 } else { 5 }).collect();
    let arrivals = trace(&delays);
    let out = play(
        JitterBuffer::adaptive(3, 12),
        &arrivals,
        400 * PACKET_MS + 500,
    );

    let expected: Vec<u16> = (0..400u16).collect();
    assert_eq!(out.sequences(), expected, "{}", out.summary());
    assert_eq!(out.late, 0, "nothing was reordered past its own slot");
    assert_eq!(out.lost, 0);
}

/// The field report, stated as a bound: sustained jitter must not turn into sustained latency.
///
/// Thirty seconds of a network that never settles. If depth ratcheted — grew on every late packet
/// and never gave any back — the hold time late in the trace would be strictly worse than early
/// in it, and at a packet per 20 ms it would reach seconds long before the trace ended.
#[test]
fn sustained_jitter_does_not_accumulate_into_latency() {
    let count = 1500usize;
    let arrivals = trace(&jitter(count, 5, 65));
    let out = play(
        JitterBuffer::adaptive(3, 12),
        &arrivals,
        count as u64 * PACKET_MS + 500,
    );

    // What the ceiling promises: a buffer of `depth` packets releases when it holds `depth`, so
    // nothing waits longer than `depth` intervals. Twelve of them is the configured maximum.
    let ceiling = 12 * PACKET_MS;
    assert!(
        out.hold_max_ms <= ceiling,
        "a packet waited {} ms, past the {ceiling} ms the depth ceiling allows: {}",
        out.hold_max_ms,
        out.summary()
    );
    assert!(
        out.depth_peak <= 12,
        "the ceiling is the ceiling: {}",
        out.summary()
    );

    // And no drift: the last third of the call costs no more than the whole of it.
    let overall = out.hold_max_after(0, &arrivals);
    let ending = out.hold_max_after(count as u64 * PACKET_MS * 2 / 3, &arrivals);
    assert!(
        ending <= overall,
        "latency grew across the call: {overall} ms overall, {ending} ms at the end: {}",
        out.summary()
    );
    assert!(
        out.hold_mean_ms() < 100.0,
        "mean hold {:.1} ms on a network whose worst packet is 65 ms late: {}",
        out.hold_mean_ms(),
        out.summary()
    );
}

/// The pathological case the ceiling exists for: a third of the packets 300 ms late, which is
/// worse than any call anyone would stay on. It still may not become seconds.
#[test]
fn even_a_hostile_network_cannot_drive_the_hold_time_past_the_ceiling() {
    let count = 1500usize;
    let delays: Vec<u64> = (0..count)
        .map(|i| if i % 3 == 0 { 300 } else { 5 })
        .collect();
    let arrivals = trace(&delays);
    let out = play(
        JitterBuffer::adaptive(3, 12),
        &arrivals,
        count as u64 * PACKET_MS + 500,
    );

    assert!(
        out.hold_max_ms < 1000,
        "the buffer must not reach a second of hold even here: {}",
        out.summary()
    );
    assert!(out.depth_peak <= 12, "{}", out.summary());
    // The measured baseline, guarded loosely enough that a different policy is not a failure and
    // tightly enough that a worse one is.
    assert!(
        out.hold_mean_ms() < 300.0,
        "mean hold {:.1} ms: {}",
        out.hold_mean_ms(),
        out.summary()
    );
}

/// One straggler three seconds late must not become three seconds of buffer.
///
/// The shape that would ratchet if lateness deepened the buffer without a ceiling: the straggler
/// is refused, the packets behind it keep their ordinary cost, and the depth it bought is given
/// back once the network has been clean for long enough to prove it.
#[test]
fn a_single_very_late_packet_is_refused_rather_than_waited_for() {
    let count = 1500usize;
    let mut delays = vec![5u64; count];
    delays[500] = 3000;
    let arrivals = trace(&delays);
    let out = play(
        JitterBuffer::adaptive(3, 12),
        &arrivals,
        count as u64 * PACKET_MS + 4000,
    );

    assert_eq!(
        out.late,
        1,
        "the straggler missed its slot: {}",
        out.summary()
    );
    assert_eq!(
        out.lost,
        1,
        "and its slot was counted as a gap: {}",
        out.summary()
    );
    assert!(
        out.hold_max_ms < 200,
        "no packet may be held while a straggler is waited for: {}",
        out.summary()
    );
    assert!(
        !out.sequences().contains(&500),
        "a packet that late must not be played out of order: {}",
        out.summary()
    );
    assert!(
        out.depth_peak > 3,
        "lateness must deepen the buffer, or there is nothing to give back: {}",
        out.summary()
    );
    assert_eq!(
        out.depth_final,
        3,
        "and a clean second half must earn all of it back: {}",
        out.summary()
    );
}

/// Duplicates are refused, not played twice, and they are not evidence of jitter.
#[test]
fn duplicates_are_refused_and_cost_nothing() {
    let count = 500usize;
    let mut arrivals = trace(&vec![5u64; count]);
    let copies: Vec<Arrival> = arrivals
        .iter()
        .filter(|a| a.sequence % 5 == 0)
        .map(|a| Arrival {
            at_ms: a.at_ms + 3,
            sequence: a.sequence,
        })
        .collect();
    let expected_duplicates = copies.len() as u64;
    arrivals.extend(copies);

    let out = play(
        JitterBuffer::adaptive(3, 12),
        &arrivals,
        count as u64 * PACKET_MS + 500,
    );

    assert_eq!(out.duplicates, expected_duplicates, "{}", out.summary());
    let expected: Vec<u16> = (0..count as u16).collect();
    assert_eq!(
        out.sequences(),
        expected,
        "each packet played exactly once: {}",
        out.summary()
    );
    assert_eq!(
        out.depth_final,
        3,
        "a duplicate is not evidence of jitter: {}",
        out.summary()
    );
}

/// The 16-bit counter wraps every ~22 minutes at 50 packets per second. Under jitter, packets
/// from either side of the wrap are in flight at once, and a buffer reading the wrap as a jump
/// backwards would refuse a minute of audio while it resynchronised.
#[test]
fn the_sequence_wrap_is_a_continuation_under_jitter() {
    let count = 600usize;
    let first = 65_300u32;
    let arrivals: Vec<Arrival> = (0..count)
        .map(|i| Arrival {
            at_ms: i as u64 * PACKET_MS + if i % 4 == 0 { 45 } else { 5 },
            sequence: ((first + i as u32) % 65_536) as u16,
        })
        .collect();

    let out = play(
        JitterBuffer::adaptive(3, 12),
        &arrivals,
        count as u64 * PACKET_MS + 500,
    );

    let expected: Vec<u16> = (0..count)
        .map(|i| ((first + i as u32) % 65_536) as u16)
        .collect();
    assert_eq!(out.sequences(), expected, "{}", out.summary());
    assert_eq!(out.late, 0, "{}", out.summary());
    assert_eq!(out.lost, 0, "the wrap is not a gap: {}", out.summary());
}

/// Loss is counted at release, not waited for — and that count is what `sipx-media` conceals
/// from. A buffer that under-counted gaps would leave the played timeline short by exactly as
/// much, which is the drift `M-45` was filed about.
#[test]
fn every_gap_is_counted_once_at_the_moment_it_is_played_over() {
    let count = 500usize;
    let arrivals: Vec<Arrival> = trace(&vec![5u64; count])
        .into_iter()
        .filter(|a| a.sequence % 11 != 0 || a.sequence == 0)
        .collect();
    let dropped = count as u64 - arrivals.len() as u64;

    let out = play(
        JitterBuffer::adaptive(3, 12),
        &arrivals,
        count as u64 * PACKET_MS + 500,
    );

    assert_eq!(
        out.lost,
        dropped,
        "every gap counted exactly once: {}",
        out.summary()
    );
    assert_eq!(out.played.len(), arrivals.len(), "{}", out.summary());
    assert!(
        out.hold_max_ms <= 12 * PACKET_MS,
        "loss must not deepen the buffer: {}",
        out.summary()
    );
}

/// A clean network pays the configured depth and not a millisecond more — the control for every
/// bound above, without which "bounded" could mean "always at the ceiling".
#[test]
fn a_clean_network_pays_exactly_the_configured_depth() {
    let count = 500usize;
    let arrivals = trace(&vec![5u64; count]);
    let out = play(
        JitterBuffer::adaptive(3, 12),
        &arrivals,
        count as u64 * PACKET_MS + 500,
    );

    assert_eq!(out.depth_final, 3, "{}", out.summary());
    assert_eq!(out.depth_peak, 3, "{}", out.summary());
    assert_eq!(out.late, 0);
    assert_eq!(out.lost, 0);
    // Three intervals of hold for a depth of three, and the tail waits out the silence flush.
    // Anything past that is latency nobody asked for.
    assert!(
        out.hold_max_ms <= 3 * PACKET_MS + FLUSH_AFTER_MS,
        "hold {} ms on a network with no jitter at all: {}",
        out.hold_max_ms,
        out.summary()
    );
}
