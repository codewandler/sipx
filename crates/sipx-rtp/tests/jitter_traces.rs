//! The adaptive buffer measured against the fixed one, on identical synthetic traces.
//!
//! This is the only kind of test that can justify an adaptive buffer existing. "It adapts" is
//! not a claim worth making; "it loses less than a constant on a bad network and costs no more
//! on a good one" is, and it is falsifiable. Both buffers see the same packets at the same
//! times and are drained by the same playout clock, so the only difference is the policy.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use bytes::Bytes;
use sipx_rtp::JitterBuffer;
use sipx_rtp::packet::Packet;

/// G.711: 8000 timestamp units per second, 160 per 20 ms packet.
const CLOCK: u32 = 8000;
const INTERVAL: u32 = 160;
const PACKET_MS: u64 = 20;

/// What one run of a trace produced.
#[derive(Debug, PartialEq, Eq)]
struct Outcome {
    /// Packets that arrived after their slot had been played. This is the audible failure the
    /// buffer exists to prevent.
    late: u64,
    /// Playout slots with nothing to play.
    underruns: u64,
    /// Packets actually handed to the consumer.
    played: u64,
    /// The depth the buffer ended on, in packets — which is the latency it was charging.
    final_depth: usize,
    /// The deepest it ever got. Without this, "it came back down" cannot be told from "it
    /// never went up", and a buffer that only ever grows would pass for one that adapts.
    peak_depth: usize,
}

/// Run a trace through a buffer with a fixed playout clock.
///
/// `delays_ms[i]` is how late packet `i` arrives relative to its ideal send time. Packets are
/// pushed at their arrival instants and the consumer pops every 20 ms once playout has begun,
/// which is what a real media session does.
fn run(mut buffer: JitterBuffer, delays_ms: &[u64]) -> Outcome {
    let mut arrivals: Vec<(u64, usize)> = delays_ms
        .iter()
        .enumerate()
        .map(|(i, delay)| (i as u64 * PACKET_MS + delay, i))
        .collect();
    arrivals.sort_unstable();

    let mut peak_depth = buffer.depth();
    let mut next_arrival = 0usize;
    let mut playout: Option<u64> = None;
    let mut underruns = 0;
    let mut played = 0;

    let horizon = delays_ms.len() as u64 * PACKET_MS + 2000;
    for now in 0..horizon {
        while next_arrival < arrivals.len() && arrivals[next_arrival].0 == now {
            let index = arrivals[next_arrival].1;
            let packet = Packet::new(
                0,
                index as u16,
                index as u32 * INTERVAL,
                1,
                Bytes::from(vec![0u8; INTERVAL as usize]),
            );
            // Arrival on the local clock, in timestamp units — the convention the RTCP
            // statistics use, so both estimates are built from the same quantity.
            let arrival = (now * u64::from(CLOCK) / 1000) as u32;
            buffer.push_at(packet, arrival);
            peak_depth = peak_depth.max(buffer.depth());
            next_arrival += 1;
        }

        match playout {
            // Playout starts the moment the buffer is willing to release anything.
            None => {
                if buffer.pop().is_some() {
                    played += 1;
                    playout = Some(now + PACKET_MS);
                }
            }
            Some(due) if now == due => {
                if buffer.pop().is_some() {
                    played += 1;
                } else {
                    underruns += 1;
                }
                playout = Some(now + PACKET_MS);
            }
            Some(_) => {}
        }
    }

    Outcome {
        late: buffer.late(),
        underruns,
        played,
        final_depth: buffer.depth(),
        peak_depth,
    }
}

/// A network that is well behaved: a constant 5 ms of delay and nothing else.
fn clean(count: usize) -> Vec<u64> {
    vec![5; count]
}

/// A network with recurring delay spikes — the shape a congested uplink actually has, rather
/// than uniform noise. A constant-depth buffer sized for the good stretches loses the spikes.
fn spiky(count: usize) -> Vec<u64> {
    (0..count)
        .map(|i| if i % 7 == 0 { 95 } else { 5 })
        .collect()
}

/// M-9's exit criterion.
#[test]
fn an_adaptive_buffer_loses_less_than_a_fixed_one_on_a_jittery_trace() {
    let trace = spiky(600);
    let fixed = run(JitterBuffer::new(2), &trace);
    let adaptive = run(JitterBuffer::adaptive(2, 12), &trace);

    assert!(
        adaptive.late < fixed.late,
        "the point of adapting: adaptive {} late vs fixed {} late",
        adaptive.late,
        fixed.late
    );
    assert!(
        adaptive.played > fixed.played,
        "fewer packets late must mean more packets played: {adaptive:?} vs {fixed:?}"
    );
    assert!(
        adaptive.final_depth > 2,
        "it must actually have grown, not merely have been lucky"
    );
}

/// And the other half, which is the one that stops "adaptive" meaning "always deeper": on a
/// clean network it costs exactly what the constant costs.
#[test]
fn an_adaptive_buffer_costs_no_more_than_a_fixed_one_on_a_clean_trace() {
    let trace = clean(600);
    let fixed = run(JitterBuffer::new(2), &trace);
    let adaptive = run(JitterBuffer::adaptive(2, 12), &trace);

    assert_eq!(
        adaptive.final_depth, 2,
        "no jitter means no reason to hold anything extra"
    );
    assert_eq!(adaptive.late, fixed.late);
    assert_eq!(adaptive.underruns, fixed.underruns);
    assert_eq!(
        adaptive.played, fixed.played,
        "identical behaviour on a network that never misbehaves"
    );
}

/// The upper bound is a real bound. A network this bad would drive an unbounded buffer to
/// seconds of latency, at which point the call is worse than one with gaps in it.
#[test]
fn depth_is_bounded_however_bad_the_network_gets() {
    let trace: Vec<u64> = (0..800).map(|i| if i % 2 == 0 { 5 } else { 900 }).collect();
    let outcome = run(JitterBuffer::adaptive(2, 6), &trace);
    assert!(
        outcome.final_depth <= 6,
        "the ceiling must hold: {outcome:?}"
    );
}

/// And the lower bound, which matters for the opposite reason: a buffer that shrank to nothing
/// on a perfect network would have no slack left for the first packet that was not perfect.
#[test]
fn depth_never_falls_below_the_floor() {
    let outcome = run(JitterBuffer::adaptive(3, 10), &clean(2000));
    assert_eq!(outcome.final_depth, 3);
}

/// Trouble, then quiet. The buffer must give the latency back — but slowly, and only after the
/// quiet has lasted long enough to be evidence rather than a gap between two spikes.
#[test]
fn the_buffer_gives_latency_back_once_the_network_settles() {
    let mut trace = spiky(300);
    trace.extend(clean(3000));

    let outcome = run(JitterBuffer::adaptive(2, 12), &trace);
    assert!(
        outcome.peak_depth > 2,
        "it must have grown during the trouble, or there is nothing to give back: {outcome:?}"
    );
    assert!(
        outcome.final_depth < outcome.peak_depth,
        "a buffer that only ever grows is not adaptive: {outcome:?}"
    );
    assert_eq!(
        outcome.final_depth, 2,
        "the quiet lasted long enough to earn all of it back: {outcome:?}"
    );

    // And shrinking really is free: everything that arrived in the quiet stretch was played,
    // so no audio was discarded to buy the latency back.
    assert!(
        outcome.played >= 3000,
        "shrinking must not throw audio away: {outcome:?}"
    );
}

/// Sanity on the harness itself. If the trace generator or the playout clock were wrong, the
/// comparisons above would be comparing two runs of the same mistake.
#[test]
fn the_harness_plays_everything_on_a_clean_network() {
    let outcome = run(JitterBuffer::new(2), &clean(500));
    assert_eq!(outcome.late, 0);
    assert!(
        outcome.played >= 498,
        "a clean network should play nearly everything: {outcome:?}"
    );
}
