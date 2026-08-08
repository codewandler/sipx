//! A jitter buffer.
//!
//! The network delivers packets late, early, twice, or not at all. Audio needs them evenly
//! spaced and in order. The buffer trades a fixed amount of latency for that, and the whole
//! design question is how much.
//!
//! Two of them, and the fixed one is not a stepping stone that got left behind. It is the
//! control: an adaptive buffer that cannot be shown to beat a constant on a bad network and to
//! match it on a good one is just a constant with extra machinery and extra ways to be wrong.
//!
//! The asymmetry that shapes the adaptive policy: **being too shallow is audible, being too
//! deep is not.** A packet that arrives after its slot was played is a gap in the audio; a
//! buffer holding one packet more than it needs is 20 ms of latency nobody notices. So it grows
//! at the first sign of trouble and shrinks only on sustained evidence that the trouble is
//! over.
//!
//! Shrinking costs nothing here, which is worth being explicit about because it is the part
//! people expect to be hard. At packet granularity, lowering the depth means the next packet is
//! released one slot sooner — it is not dropped, and nothing is played faster. Time-scale
//! modification of the audio belongs in the media layer; this layer removes latency simply by
//! holding less of it.

use std::collections::BTreeMap;

use crate::packet::{Packet, sequence_is_newer};

/// How long a stretch of clean network is needed before the buffer gives up a packet of depth.
///
/// 250 packets is five seconds at the usual 20 ms. Long because shrinking is a bet that the
/// network has settled, and losing that bet is audible while winning it saves 20 ms nobody
/// notices. A shorter window would have the buffer shrink in the quiet between two jitter
/// spikes and be caught out by the second.
const SHRINK_AFTER: u32 = 250;

/// How much lateness a buffer of this depth can absorb, in timestamp units.
///
/// [`JitterBuffer::pop`] holds `depth` packets before releasing, so a packet arriving up to
/// `depth - 1` intervals behind its neighbours still makes its slot. That relation is the whole
/// basis for choosing a depth, and writing it once means the release rule and the sizing rule
/// cannot drift apart.
fn absorbable(depth: usize, interval: f64) -> f64 {
    let intervals = u32::try_from(depth.saturating_sub(1)).unwrap_or(u32::MAX);
    f64::from(intervals) * interval
}

/// How the buffer chooses its depth.
#[derive(Debug, Clone, Copy)]
enum Policy {
    /// A constant, chosen by the caller.
    Fixed,
    /// Between two bounds, from observed jitter and lateness.
    Adaptive {
        /// Never shallower than this, however clean the network looks.
        min: usize,
        /// Never deeper, however bad it gets. A pathological network must not be able to drive
        /// latency without limit: at some point a call with three seconds of delay is worse
        /// than a call with gaps, and the caller is entitled to decide where that point is.
        max: usize,
    },
}

/// Buffers packets, reorders them, and reports what went missing.
#[derive(Debug)]
pub struct JitterBuffer {
    /// How many packets to hold before releasing.
    depth: usize,
    packets: BTreeMap<u64, Packet>,
    /// The extended sequence number of the last packet released.
    last_released: Option<u64>,
    /// The high 48 bits, tracking wraps of the 16-bit counter.
    cycles: u64,
    highest: Option<u16>,
    received: u64,
    lost: u64,
    duplicates: u64,
    late: u64,
    policy: Policy,
    /// Smoothed interarrival jitter, in timestamp units.
    jitter: f64,
    /// The previous packet's transit time, which is what jitter is the change in.
    last_transit: Option<u32>,
    /// The packetisation interval in timestamp units, learned from the stream rather than
    /// assumed: 20 ms is usual, 30 ms is common, and a buffer that assumed one and got the
    /// other would size itself half or twice as deep as it meant to.
    interval: Option<u32>,
    last_timestamp: Option<u32>,
    /// How many packets in a row have wanted a shallower buffer than the one we have.
    clean_run: u32,
}

impl JitterBuffer {
    /// A buffer holding `depth` packets.
    ///
    /// Depth is in packets rather than milliseconds because the buffer does not know the
    /// packetisation interval; at the usual 20 ms, a depth of 3 is 60 ms of added latency.
    #[must_use]
    pub fn new(depth: usize) -> Self {
        Self {
            depth: depth.max(1),
            packets: BTreeMap::new(),
            last_released: None,
            cycles: 0,
            highest: None,
            received: 0,
            lost: 0,
            duplicates: 0,
            late: 0,
            policy: Policy::Fixed,
            jitter: 0.0,
            last_transit: None,
            interval: None,
            last_timestamp: None,
            clean_run: 0,
        }
    }

    /// A buffer that sizes itself, between `min` and `max` packets.
    ///
    /// It starts at `min` and stays there until it has evidence it needs more, so a clean
    /// network pays exactly what the fixed buffer would. Feed it with [`Self::push_at`]:
    /// [`Self::push`] carries no arrival time, and a buffer with no arrival times cannot
    /// measure jitter and will never adapt.
    #[must_use]
    pub fn adaptive(min: usize, max: usize) -> Self {
        let min = min.max(1);
        let max = max.max(min);
        Self {
            policy: Policy::Adaptive { min, max },
            ..Self::new(min)
        }
    }

    /// Accept a packet, noting when it arrived.
    ///
    /// `arrival` is the local clock in the same units as the RTP timestamp — for G.711, 8000
    /// per second. The same convention as [`crate::rtcp::StreamStats::on_packet`], and for the
    /// same reason: mixing units is how a jitter estimate becomes a number that means nothing.
    ///
    /// `false` means the packet was refused and its audio will never be played. That is a
    /// discard, and a caller in a media path owes it a counter — so the answer is `#[must_use]`
    /// rather than something a later author can drop without the compiler saying so.
    #[must_use = "a refused packet is a discard the caller has to account for"]
    pub fn push_at(&mut self, packet: Packet, arrival: u32) -> bool {
        let (timestamp, was_late) = (packet.timestamp, self.late);
        self.observe(timestamp, arrival);
        let kept = self.push(packet);
        if self.late > was_late {
            // The most direct evidence there is that the buffer is too shallow: a packet turned
            // up after its slot had been played. Nothing else needs to be inferred.
            self.deepen();
        } else {
            self.resize();
        }
        kept
    }

    /// How deep the buffer currently is, in packets.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// Smoothed interarrival jitter, in timestamp units. Zero under a fixed policy.
    #[must_use]
    pub fn jitter(&self) -> f64 {
        self.jitter
    }

    /// Update the jitter estimate and the packetisation interval from one arrival.
    fn observe(&mut self, timestamp: u32, arrival: u32) {
        if let Some(previous) = self.last_timestamp {
            let delta = timestamp.wrapping_sub(previous);
            // A plausible interval only. Zero is a second packet of the same frame (DTMF does
            // this); a huge delta is the gap after a silence, and taking either for the
            // packetisation interval would size the buffer from noise.
            if delta > 0 && delta < 48_000 {
                self.interval = Some(match self.interval {
                    // Smoothed, so one late-arriving reorder does not redefine the interval.
                    Some(current) => (current * 7 + delta) / 8,
                    None => delta,
                });
            }
        }
        self.last_timestamp = Some(timestamp);

        // RFC 3550 §A.8, the same recurrence the RTCP statistics use. Modular arithmetic
        // throughout: the timestamp starts at a random value, so either clock wrapping mid-call
        // is ordinary, and widening the subtraction turns each wrap into phantom jitter.
        let transit = arrival.wrapping_sub(timestamp);
        if let Some(previous) = self.last_transit {
            let difference = transit.wrapping_sub(previous).cast_signed().unsigned_abs();
            self.jitter += (f64::from(difference) - self.jitter) / 16.0;
        }
        self.last_transit = Some(transit);
    }

    /// The depth the current estimate asks for.
    fn wanted(&self) -> Option<usize> {
        let Policy::Adaptive { min, max } = self.policy else {
            return None;
        };
        let interval = self.interval?;
        // Derived from the release rule rather than guessed at. `pop` holds `depth` packets, so
        // a packet arriving up to `depth - 1` intervals late still makes its slot; turn that
        // round and absorbing a deviation of `d` needs `d / interval + 1` packets.
        //
        // The deviation is `2 * jitter` because `jitter` is a smoothed *mean* deviation and not
        // a maximum. Covering one mean would leave roughly half of a bad network's packets
        // late, and covering the worst arrival ever seen would let one outlier set the latency
        // for the rest of the call.
        //
        // Note that this floors at `min` on its own: with no jitter the expression is 1, and
        // the clamp does the rest. An earlier version added the slack *to* `min`, which looked
        // equivalent and was not — `ceil` of the floating-point residue left by the decaying
        // average is 1, not 0, so the buffer sat permanently one packet deeper than the network
        // ever asked for and never gave that packet back.
        let deviation = 2.0 * self.jitter;
        let interval = f64::from(interval);
        // Searched rather than divided-and-rounded. The candidates are a handful of small
        // integers, so a scan is cheap, and it keeps every number that ends up as a depth in
        // `usize` from the start — no float-to-integer conversion whose behaviour at the ends
        // has to be reasoned about, and no rounding rule to get subtly wrong.
        Some(
            (min..=max)
                .find(|&depth| deviation <= absorbable(depth, interval))
                .unwrap_or(max),
        )
    }

    /// Grow now. Growing is never delayed: the evidence for it is already audible.
    fn deepen(&mut self) {
        if let Policy::Adaptive { max, .. } = self.policy {
            self.depth = (self.depth + 1).min(max);
            self.clean_run = 0;
        }
    }

    /// Grow to what the estimate asks for, or shrink after a long enough clean stretch.
    fn resize(&mut self) {
        let Some(want) = self.wanted() else {
            return;
        };
        if want > self.depth {
            self.depth = want;
            self.clean_run = 0;
            return;
        }
        if want < self.depth {
            self.clean_run += 1;
            if self.clean_run >= SHRINK_AFTER {
                self.depth -= 1;
                self.clean_run = 0;
            }
            return;
        }
        self.clean_run = 0;
    }

    /// Accept a packet.
    ///
    /// Returns whether it was kept. A packet already released is refused: playing it would put
    /// audio out of order, which is worse than the gap it was going to fill.
    pub fn push(&mut self, packet: Packet) -> bool {
        self.received += 1;
        let extended = self.extend(packet.sequence);

        if let Some(last) = self.last_released
            && extended <= last
        {
            // It arrived after its slot had already been played.
            self.late += 1;
            return false;
        }
        if self.packets.contains_key(&extended) {
            self.duplicates += 1;
            return false;
        }

        self.packets.insert(extended, packet);
        true
    }

    /// Take the next packet, if the buffer has filled enough to release one.
    ///
    /// Returns `None` while still filling — that is the latency being paid for, not an error.
    pub fn pop(&mut self) -> Option<Packet> {
        if self.packets.len() < self.depth {
            return None;
        }
        let &next = self.packets.keys().next()?;

        if let Some(last) = self.last_released {
            let expected = last + 1;
            if next > expected {
                // The packet we wanted never came. Count it and move on: waiting longer only
                // adds latency, since anything that late is useless anyway.
                self.lost += next - expected;
            }
        }

        let packet = self.packets.remove(&next)?;
        self.last_released = Some(next);
        Some(packet)
    }

    /// Release everything held, in order, regardless of depth.
    pub fn drain(&mut self) -> Vec<Packet> {
        let mut out = Vec::with_capacity(self.packets.len());
        while let Some(&next) = self.packets.keys().next() {
            if let Some(packet) = self.packets.remove(&next) {
                self.last_released = Some(next);
                out.push(packet);
            }
        }
        out
    }

    /// How many packets are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.packets.len()
    }

    /// Whether nothing is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }

    /// How many packets arrived.
    #[must_use]
    pub fn received(&self) -> u64 {
        self.received
    }

    /// How many never arrived, as counted at release time.
    #[must_use]
    pub fn lost(&self) -> u64 {
        self.lost
    }

    /// How many arrived more than once.
    #[must_use]
    pub fn duplicates(&self) -> u64 {
        self.duplicates
    }

    /// How many arrived after their slot had been played.
    #[must_use]
    pub fn late(&self) -> u64 {
        self.late
    }

    /// Map a 16-bit sequence number onto a monotonic 64-bit one.
    ///
    /// This is what makes reordering across the wrap work: once numbers are extended, ordinary
    /// comparison is correct again, and the buffer's `BTreeMap` sorts them properly.
    fn extend(&mut self, sequence: u16) -> u64 {
        match self.highest {
            None => {
                self.highest = Some(sequence);
                // The origin starts one cycle up, not at zero. The stream begins at a
                // random sequence (RFC 3550 §5.1), so the first arrival can be from just
                // after a wrap — and a straggler from before it must extend *below* the
                // base, which needs room underneath.
                self.cycles = 1;
                65_536 + u64::from(sequence)
            }
            Some(highest) => {
                if sequence_is_newer(sequence, highest) {
                    // A newer number that is numerically smaller means the counter wrapped.
                    if sequence < highest {
                        self.cycles += 1;
                    }
                    self.highest = Some(sequence);
                    self.cycles * 65_536 + u64::from(sequence)
                } else {
                    // Older, and possibly from before a wrap we have already counted.
                    if sequence > highest && self.cycles > 0 {
                        (self.cycles - 1) * 65_536 + u64::from(sequence)
                    } else {
                        self.cycles * 65_536 + u64::from(sequence)
                    }
                }
            }
        }
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
    use bytes::Bytes;

    fn packet(sequence: u16) -> Packet {
        Packet::new(
            0,
            sequence,
            u32::from(sequence) * 160,
            1,
            Bytes::from(vec![u8::try_from(sequence % 256).unwrap_or(0); 160]),
        )
    }

    fn sequences(packets: &[Packet]) -> Vec<u16> {
        packets.iter().map(|p| p.sequence).collect()
    }

    /// Packets come out in order, and the buffer keeps `depth - 1` in hand. That reserve is
    /// the whole point: it is the slack available to absorb the next late arrival, and a
    /// buffer that drained itself completely would have none.
    #[test]
    fn packets_in_order_come_out_in_order_leaving_a_reserve() {
        let mut buffer = JitterBuffer::new(3);
        for sequence in 1..=6 {
            buffer.push(packet(sequence));
        }
        let mut out = Vec::new();
        while let Some(packet) = buffer.pop() {
            out.push(packet);
        }
        assert_eq!(sequences(&out), vec![1, 2, 3, 4]);
        assert_eq!(buffer.len(), 2, "depth - 1 stays held as slack");
        assert_eq!(buffer.lost(), 0);

        // And the reserve is released when the stream ends.
        assert_eq!(sequences(&buffer.drain()), vec![5, 6]);
    }

    /// The buffer exists for this: packets arriving out of order are played in order.
    #[test]
    fn reordered_packets_are_played_in_order() {
        let mut buffer = JitterBuffer::new(3);
        for sequence in [3, 1, 2, 5, 4, 6] {
            buffer.push(packet(sequence));
        }
        assert_eq!(sequences(&buffer.drain()), vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn nothing_is_released_until_the_buffer_has_filled() {
        let mut buffer = JitterBuffer::new(3);
        buffer.push(packet(1));
        assert!(buffer.pop().is_none(), "still filling");
        buffer.push(packet(2));
        assert!(buffer.pop().is_none());
        buffer.push(packet(3));
        assert!(buffer.pop().is_some(), "now it releases");
    }

    #[test]
    fn a_duplicate_is_counted_and_dropped() {
        let mut buffer = JitterBuffer::new(2);
        assert!(buffer.push(packet(1)));
        assert!(!buffer.push(packet(1)), "the second copy is refused");
        assert_eq!(buffer.duplicates(), 1);
        assert_eq!(buffer.len(), 1);
    }

    /// A gap is counted at release time rather than waited for. Waiting only adds latency,
    /// since a packet that late is useless anyway.
    #[test]
    fn a_missing_packet_is_counted_as_lost() {
        let mut buffer = JitterBuffer::new(2);
        for sequence in [1, 2, 4, 5] {
            buffer.push(packet(sequence));
        }
        let out = buffer.drain();
        assert_eq!(sequences(&out), vec![1, 2, 4, 5]);
        // Draining does not diagnose gaps; popping does.
        let mut buffer = JitterBuffer::new(1);
        for sequence in [1, 2, 4] {
            buffer.push(packet(sequence));
        }
        buffer.pop();
        buffer.pop();
        buffer.pop();
        assert_eq!(buffer.lost(), 1, "3 never arrived");
    }

    /// A packet whose slot has already been played is refused. Playing it would put audio out
    /// of order, which sounds worse than the gap it was going to fill.
    #[test]
    fn a_packet_that_arrives_too_late_is_refused() {
        let mut buffer = JitterBuffer::new(1);
        buffer.push(packet(1));
        buffer.push(packet(2));
        buffer.pop();
        buffer.pop();

        assert!(!buffer.push(packet(1)), "its slot has been played");
        assert_eq!(buffer.late(), 1);
    }

    /// The counter wraps every ~22 minutes at 50 packets per second. A buffer that treats the
    /// wrap as a jump backwards discards a minute of audio while it resynchronises.
    #[test]
    fn the_buffer_orders_correctly_across_a_sequence_wrap() {
        let mut buffer = JitterBuffer::new(2);
        for sequence in [65_533, 65_534, 65_535, 0, 1, 2] {
            buffer.push(packet(sequence));
        }
        assert_eq!(
            sequences(&buffer.drain()),
            vec![65_533, 65_534, 65_535, 0, 1, 2],
            "the wrap is a continuation, not a jump backwards"
        );
        assert_eq!(buffer.lost(), 0);
    }

    /// The sequence starts at a random value (RFC 3550 §5.1), so a stream can begin just
    /// short of the 16-bit wrap — and the first packet to arrive can be from just *after*
    /// it. A straggler from before the wrap must then sort before the base, not be mapped
    /// ~65000 slots into the future.
    #[test]
    fn a_pre_wrap_straggler_at_stream_start_sorts_before_the_base() {
        let mut buffer = JitterBuffer::new(2);
        for sequence in [0, 65_535, 1, 2, 3] {
            buffer.push(packet(sequence));
        }
        assert_eq!(sequences(&buffer.drain()), vec![65_535, 0, 1, 2, 3]);
    }

    /// The catastrophic form of the same mistake: the straggler's slot has already been
    /// played, and mapping it into the future poisons `last_released` — after which every
    /// genuine packet is refused as late and the stream is silent until the real wrap.
    #[test]
    fn a_pre_wrap_straggler_cannot_mute_the_stream() {
        let mut buffer = JitterBuffer::new(1);
        buffer.push(packet(0));
        assert!(buffer.pop().is_some());

        assert!(
            !buffer.push(packet(65_535)),
            "its slot is in the past, so it is late — not 65535 slots early"
        );
        assert_eq!(buffer.late(), 1);

        // And the genuine stream keeps flowing.
        for sequence in [1, 2, 3] {
            assert!(buffer.push(packet(sequence)));
        }
        assert_eq!(sequences(&buffer.drain()), vec![1, 2, 3]);
    }

    /// Reordering *across* the wrap is the hard case: a packet from before the wrap arriving
    /// after one from after it.
    #[test]
    fn reordering_across_the_wrap_still_sorts() {
        let mut buffer = JitterBuffer::new(4);
        for sequence in [65_535, 1, 0, 2] {
            buffer.push(packet(sequence));
        }
        assert_eq!(sequences(&buffer.drain()), vec![65_535, 0, 1, 2]);
    }

    #[test]
    fn statistics_add_up() {
        let mut buffer = JitterBuffer::new(1);
        for sequence in [1, 2, 2, 4] {
            buffer.push(packet(sequence));
        }
        assert_eq!(buffer.received(), 4);
        assert_eq!(buffer.duplicates(), 1);
        while buffer.pop().is_some() {}
        assert_eq!(buffer.lost(), 1);
    }
}
