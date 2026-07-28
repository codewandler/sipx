//! A jitter buffer.
//!
//! The network delivers packets late, early, twice, or not at all. Audio needs them evenly
//! spaced and in order. The buffer trades a fixed amount of latency for that, and the whole
//! design question is how much.
//!
//! What this one does *not* do is adapt its depth. An adaptive buffer is better on a bad
//! network and much harder to reason about, and getting the fixed case right first means the
//! adaptive one has something to be measured against.

use std::collections::BTreeMap;

use crate::packet::{Packet, sequence_is_newer};

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
        }
    }

    /// Accept a packet.
    ///
    /// Returns whether it was kept. A packet already released is refused: playing it would put
    /// audio out of order, which is worse than the gap it was going to fill.
    pub fn push(&mut self, packet: Packet) -> bool {
        self.received += 1;
        let extended = self.extend(packet.sequence);

        if let Some(last) = self.last_released {
            if extended <= last {
                // It arrived after its slot had already been played.
                self.late += 1;
                return false;
            }
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
                self.cycles = 0;
                u64::from(sequence)
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
