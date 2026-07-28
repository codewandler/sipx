//! RTCP sender and receiver reports (RFC 3550 §6.4).
//!
//! RTP carries the media; RTCP carries what happened to it. Without reports a stack cannot say
//! why a call sounded bad, and cannot answer a peer that asks.
//!
//! The field that repays attention is **interarrival jitter**. It is not a variance and not a
//! standard deviation — it is the smoothed mean deviation of packet spacing, updated per
//! packet by the RFC's own recurrence:
//!
//! ```text
//! J += (|D(i-1, i)| - J) / 16
//! ```
//!
//! An implementation that computes a variance instead produces numbers that look plausible,
//! move in the right direction, and are wrong by a factor that depends on the traffic — which
//! is worse than reporting nothing, because someone will tune a jitter buffer with them.

use bytes::{BufMut, Bytes, BytesMut};

use crate::packet::sequence_is_newer;

/// RTCP packet types (RFC 3550 §12.1).
pub const SENDER_REPORT: u8 = 200;
/// A receiver report.
pub const RECEIVER_REPORT: u8 = 201;
/// A source description.
pub const SOURCE_DESCRIPTION: u8 = 202;
/// Goodbye.
pub const GOODBYE: u8 = 203;

/// What can go wrong reading RTCP.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RtcpError {
    /// Fewer bytes than a header.
    #[error("packet is {0} bytes; an RTCP header is 4")]
    TooShort(usize),
    /// A version other than 2.
    #[error("RTCP version {0}; only version 2 exists")]
    BadVersion(u8),
    /// The length field claims more than the packet holds.
    #[error("the length field claims more than the packet contains")]
    Truncated,
}

/// One report block: what one source's stream looked like from here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReportBlock {
    /// Whose stream this describes.
    pub ssrc: u32,
    /// Loss since the last report, as a fraction in 256ths.
    pub fraction_lost: u8,
    /// Packets lost since the stream began. 24-bit and signed, because duplicates can make it
    /// go down.
    pub cumulative_lost: i32,
    /// The highest sequence number seen, with the wrap count in the high 16 bits.
    pub extended_highest_sequence: u32,
    /// Interarrival jitter, in timestamp units.
    pub jitter: u32,
    /// The middle 32 bits of the last sender report's NTP timestamp.
    pub last_sender_report: u32,
    /// How long since that report arrived, in 1/65536 second units.
    pub delay_since_last_sender_report: u32,
}

/// A sender report: what we have sent, plus what we have received from others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenderReport {
    /// Our synchronisation source.
    pub ssrc: u32,
    /// Wallclock time, as an NTP timestamp.
    pub ntp_timestamp: u64,
    /// The RTP timestamp corresponding to it, which is what lets a receiver relate the two
    /// clocks and synchronise streams.
    pub rtp_timestamp: u32,
    /// Packets sent since the stream began.
    pub packet_count: u32,
    /// Payload bytes sent, headers excluded.
    pub octet_count: u32,
    /// What we have received from other sources.
    pub reports: Vec<ReportBlock>,
}

/// A receiver report: what we have received, from a participant that sends nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiverReport {
    /// Our synchronisation source.
    pub ssrc: u32,
    /// What we have received.
    pub reports: Vec<ReportBlock>,
}

/// An RTCP packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rtcp {
    /// A sender report.
    Sender(SenderReport),
    /// A receiver report.
    Receiver(ReceiverReport),
    /// Something else — source descriptions, goodbyes, application data.
    ///
    /// Kept rather than dropped: RTCP arrives as compound packets, and an element that
    /// discards the types it does not model cannot forward one intact.
    Other {
        /// The packet type.
        packet_type: u8,
        /// Its body, verbatim.
        payload: Bytes,
    },
}

fn put_header(out: &mut BytesMut, count: u8, packet_type: u8, body_words: u16) {
    out.put_u8(0b1000_0000 | (count & 0x1F));
    out.put_u8(packet_type);
    // The length is in 32-bit words *minus one*, counting the header. Off-by-one here shifts
    // every later packet in a compound, which is why it is written out rather than inlined.
    out.put_u16(body_words);
}

fn put_block(out: &mut BytesMut, block: &ReportBlock) {
    out.put_u32(block.ssrc);
    out.put_u8(block.fraction_lost);
    let lost = block.cumulative_lost.clamp(-0x0080_0000, 0x007F_FFFF);
    let lost = u32::from_ne_bytes(lost.to_ne_bytes()) & 0x00FF_FFFF;
    out.put_u8(u8::try_from((lost >> 16) & 0xFF).unwrap_or(0));
    out.put_u16(u16::try_from(lost & 0xFFFF).unwrap_or(0));
    out.put_u32(block.extended_highest_sequence);
    out.put_u32(block.jitter);
    out.put_u32(block.last_sender_report);
    out.put_u32(block.delay_since_last_sender_report);
}

fn read_block(bytes: &[u8], at: usize) -> Option<ReportBlock> {
    let slice = bytes.get(at..at + 24)?;
    // Byte 4 is the fraction; the cumulative count is bytes 5 to 7. Reading from 4 folds the
    // fraction into the high byte of the loss, which turns 42 lost into 1.7 million.
    let lost_raw = u32::from(*slice.get(5)?) << 16
        | u32::from(*slice.get(6)?) << 8
        | u32::from(*slice.get(7)?);
    // 24-bit two's complement, sign-extended.
    let cumulative_lost = if lost_raw & 0x0080_0000 == 0 {
        i32::try_from(lost_raw).unwrap_or(0)
    } else {
        i32::try_from(lost_raw).unwrap_or(0) - 0x0100_0000
    };
    Some(ReportBlock {
        ssrc: u32::from_be_bytes(slice.get(0..4)?.try_into().ok()?),
        fraction_lost: *slice.get(4)?,
        cumulative_lost,
        extended_highest_sequence: u32::from_be_bytes(slice.get(8..12)?.try_into().ok()?),
        jitter: u32::from_be_bytes(slice.get(12..16)?.try_into().ok()?),
        last_sender_report: u32::from_be_bytes(slice.get(16..20)?.try_into().ok()?),
        delay_since_last_sender_report: u32::from_be_bytes(slice.get(20..24)?.try_into().ok()?),
    })
}

impl Rtcp {
    /// Serialize one packet.
    #[must_use]
    pub fn encode(&self) -> Bytes {
        let mut out = BytesMut::with_capacity(64);
        match self {
            Self::Sender(report) => {
                let count = u8::try_from(report.reports.len().min(31)).unwrap_or(0);
                let words = 6 + u16::from(count) * 6;
                put_header(&mut out, count, SENDER_REPORT, words);
                out.put_u32(report.ssrc);
                out.put_u64(report.ntp_timestamp);
                out.put_u32(report.rtp_timestamp);
                out.put_u32(report.packet_count);
                out.put_u32(report.octet_count);
                for block in report.reports.iter().take(31) {
                    put_block(&mut out, block);
                }
            }
            Self::Receiver(report) => {
                let count = u8::try_from(report.reports.len().min(31)).unwrap_or(0);
                let words = 1 + u16::from(count) * 6;
                put_header(&mut out, count, RECEIVER_REPORT, words);
                out.put_u32(report.ssrc);
                for block in report.reports.iter().take(31) {
                    put_block(&mut out, block);
                }
            }
            Self::Other {
                packet_type,
                payload,
            } => {
                let words = u16::try_from(payload.len() / 4).unwrap_or(0);
                put_header(&mut out, 0, *packet_type, words);
                out.put_slice(payload);
            }
        }
        out.freeze()
    }

    /// Parse a compound packet into its parts.
    ///
    /// RTCP is sent compound — a report followed by a source description, usually — and a
    /// parser that reads only the first packet sees a fraction of what arrived.
    pub fn decode_compound(bytes: &Bytes) -> Result<Vec<Self>, RtcpError> {
        let mut packets = Vec::new();
        let mut offset = 0usize;

        while offset + 4 <= bytes.len() {
            let first = bytes.get(offset).copied().unwrap_or(0);
            let version = first >> 6;
            if version != 2 {
                return Err(RtcpError::BadVersion(version));
            }
            let count = usize::from(first & 0x1F);
            let packet_type = bytes.get(offset + 1).copied().unwrap_or(0);
            let words = usize::from(u16::from_be_bytes([
                bytes.get(offset + 2).copied().ok_or(RtcpError::Truncated)?,
                bytes.get(offset + 3).copied().ok_or(RtcpError::Truncated)?,
            ]));
            // Length is words-minus-one and excludes the first word, so the whole packet is
            // (words + 1) * 4 bytes.
            let total = (words + 1) * 4;
            let body = bytes
                .get(offset + 4..offset + total)
                .ok_or(RtcpError::Truncated)?;

            packets.push(Self::decode_one(packet_type, count, body, bytes, offset)?);
            offset += total;
        }

        if packets.is_empty() {
            return Err(RtcpError::TooShort(bytes.len()));
        }
        Ok(packets)
    }

    fn decode_one(
        packet_type: u8,
        count: usize,
        body: &[u8],
        whole: &Bytes,
        offset: usize,
    ) -> Result<Self, RtcpError> {
        match packet_type {
            SENDER_REPORT => {
                let ssrc = u32::from_be_bytes(
                    body.get(0..4)
                        .and_then(|s| s.try_into().ok())
                        .ok_or(RtcpError::Truncated)?,
                );
                let ntp = u64::from_be_bytes(
                    body.get(4..12)
                        .and_then(|s| s.try_into().ok())
                        .ok_or(RtcpError::Truncated)?,
                );
                let rtp = u32::from_be_bytes(
                    body.get(12..16)
                        .and_then(|s| s.try_into().ok())
                        .ok_or(RtcpError::Truncated)?,
                );
                let packets = u32::from_be_bytes(
                    body.get(16..20)
                        .and_then(|s| s.try_into().ok())
                        .ok_or(RtcpError::Truncated)?,
                );
                let octets = u32::from_be_bytes(
                    body.get(20..24)
                        .and_then(|s| s.try_into().ok())
                        .ok_or(RtcpError::Truncated)?,
                );
                let mut reports = Vec::with_capacity(count);
                for index in 0..count {
                    reports.push(read_block(body, 24 + index * 24).ok_or(RtcpError::Truncated)?);
                }
                Ok(Self::Sender(SenderReport {
                    ssrc,
                    ntp_timestamp: ntp,
                    rtp_timestamp: rtp,
                    packet_count: packets,
                    octet_count: octets,
                    reports,
                }))
            }
            RECEIVER_REPORT => {
                let ssrc = u32::from_be_bytes(
                    body.get(0..4)
                        .and_then(|s| s.try_into().ok())
                        .ok_or(RtcpError::Truncated)?,
                );
                let mut reports = Vec::with_capacity(count);
                for index in 0..count {
                    reports.push(read_block(body, 4 + index * 24).ok_or(RtcpError::Truncated)?);
                }
                Ok(Self::Receiver(ReceiverReport { ssrc, reports }))
            }
            other => Ok(Self::Other {
                packet_type: other,
                payload: whole.slice(offset + 4..offset + 4 + body.len()),
            }),
        }
    }
}

/// Tracks what a stream looked like from here, so a report can be produced.
///
/// The counters are the interesting part. Loss is *inferred* — nothing announces a lost packet
/// — from the difference between how many sequence numbers went by and how many packets
/// arrived. That is why duplicates can make cumulative loss negative, and why the field is
/// signed.
#[derive(Debug)]
pub struct StreamStats {
    ssrc: u32,
    /// The first sequence number seen, to count from.
    base_sequence: Option<u16>,
    /// Wrap count in the high 16 bits.
    cycles: u32,
    highest_sequence: u16,
    received: u64,
    /// What `received` was at the last report, so a fraction can be computed.
    received_at_last_report: u64,
    expected_at_last_report: u64,
    /// The smoothed interarrival jitter estimate.
    jitter: f64,
    /// The last packet's transit time, for the difference the estimate is built from.
    last_transit: Option<i64>,
}

impl StreamStats {
    /// Statistics for one source.
    #[must_use]
    pub fn new(ssrc: u32) -> Self {
        Self {
            ssrc,
            base_sequence: None,
            cycles: 0,
            highest_sequence: 0,
            received: 0,
            received_at_last_report: 0,
            expected_at_last_report: 0,
            jitter: 0.0,
            last_transit: None,
        }
    }

    /// Record an arrival.
    ///
    /// `arrival` is the local clock in the same units as the RTP timestamp — for G.711, 8000
    /// per second. Mixing units here is the other way to make jitter meaningless.
    pub fn on_packet(&mut self, sequence: u16, rtp_timestamp: u32, arrival: u32) {
        self.received += 1;

        match self.base_sequence {
            None => {
                self.base_sequence = Some(sequence);
                self.highest_sequence = sequence;
            }
            Some(_) => {
                if sequence_is_newer(sequence, self.highest_sequence) {
                    if sequence < self.highest_sequence {
                        self.cycles = self.cycles.wrapping_add(1);
                    }
                    self.highest_sequence = sequence;
                }
            }
        }

        // RFC 3550 §A.8. The transit time is the difference between the local clock and the
        // sender's; its *change* between packets is what jitter measures, so a constant offset
        // between the two clocks cancels and never appears in the estimate.
        let transit = i64::from(arrival) - i64::from(rtp_timestamp);
        if let Some(previous) = self.last_transit {
            let difference = (transit - previous).abs();
            // The RFC's recurrence, not a variance: J += (|D| - J) / 16.
            #[allow(clippy::cast_precision_loss)]
            let d = difference as f64;
            self.jitter += (d - self.jitter) / 16.0;
        }
        self.last_transit = Some(transit);
    }

    /// The highest sequence number seen, with its wrap count.
    #[must_use]
    pub fn extended_highest_sequence(&self) -> u32 {
        (self.cycles << 16) | u32::from(self.highest_sequence)
    }

    /// How many packets should have arrived.
    #[must_use]
    pub fn expected(&self) -> u64 {
        let Some(base) = self.base_sequence else {
            return 0;
        };
        u64::from(self.extended_highest_sequence()).saturating_sub(u64::from(base)) + 1
    }

    /// How many never did. Signed, because duplicates can make it negative.
    #[must_use]
    pub fn cumulative_lost(&self) -> i64 {
        i64::try_from(self.expected()).unwrap_or(0) - i64::try_from(self.received).unwrap_or(0)
    }

    /// The current jitter estimate, in timestamp units.
    #[must_use]
    pub fn jitter(&self) -> u32 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let jitter = self.jitter as u32;
        jitter
    }

    /// Produce a report block, and reset the per-interval counters.
    ///
    /// The fraction is loss *since the last report*, not since the stream began — a call that
    /// lost heavily at the start and is now clean must report clean, or nobody can see it
    /// recover.
    pub fn report_block(&mut self) -> ReportBlock {
        let expected = self.expected();
        let expected_interval = expected.saturating_sub(self.expected_at_last_report);
        let received_interval = self.received.saturating_sub(self.received_at_last_report);
        let lost_interval = i64::try_from(expected_interval).unwrap_or(0)
            - i64::try_from(received_interval).unwrap_or(0);

        let fraction = if expected_interval == 0 || lost_interval <= 0 {
            0
        } else {
            let scaled = (lost_interval * 256) / i64::try_from(expected_interval).unwrap_or(1);
            u8::try_from(scaled.clamp(0, 255)).unwrap_or(0)
        };

        self.expected_at_last_report = expected;
        self.received_at_last_report = self.received;

        ReportBlock {
            ssrc: self.ssrc,
            fraction_lost: fraction,
            cumulative_lost: i32::try_from(self.cumulative_lost()).unwrap_or(0),
            extended_highest_sequence: self.extended_highest_sequence(),
            jitter: self.jitter(),
            last_sender_report: 0,
            delay_since_last_sender_report: 0,
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

    fn block() -> ReportBlock {
        ReportBlock {
            ssrc: 0x1234_5678,
            fraction_lost: 26,
            cumulative_lost: 42,
            extended_highest_sequence: 0x0001_0064,
            jitter: 17,
            last_sender_report: 0xAABB_CCDD,
            delay_since_last_sender_report: 65_536,
        }
    }

    #[test]
    fn a_receiver_report_round_trips() {
        let report = Rtcp::Receiver(ReceiverReport {
            ssrc: 0xDEAD_BEEF,
            reports: vec![block()],
        });
        let decoded = Rtcp::decode_compound(&report.encode()).expect("decodes");
        assert_eq!(decoded, vec![report]);
    }

    #[test]
    fn a_sender_report_round_trips() {
        let report = Rtcp::Sender(SenderReport {
            ssrc: 0xCAFE_BABE,
            ntp_timestamp: 0x0123_4567_89AB_CDEF,
            rtp_timestamp: 160_000,
            packet_count: 500,
            octet_count: 80_000,
            reports: vec![block(), block()],
        });
        let decoded = Rtcp::decode_compound(&report.encode()).expect("decodes");
        assert_eq!(decoded, vec![report]);
    }

    /// Cumulative loss is 24-bit and signed, because duplicates can make it negative. A parser
    /// that reads it unsigned turns "we received three extra" into eight million lost.
    #[test]
    fn negative_cumulative_loss_survives_the_round_trip() {
        let report = Rtcp::Receiver(ReceiverReport {
            ssrc: 1,
            reports: vec![ReportBlock {
                cumulative_lost: -3,
                ..block()
            }],
        });
        let decoded = Rtcp::decode_compound(&report.encode()).expect("decodes");
        match &decoded[0] {
            Rtcp::Receiver(receiver) => assert_eq!(receiver.reports[0].cumulative_lost, -3),
            other => panic!("expected a receiver report, got {other:?}"),
        }
    }

    /// RTCP arrives compound. A parser that reads only the first packet sees a fraction of
    /// what arrived.
    #[test]
    fn a_compound_packet_is_read_as_a_whole() {
        let mut bytes = BytesMut::new();
        bytes.put_slice(
            &Rtcp::Sender(SenderReport {
                ssrc: 1,
                ntp_timestamp: 0,
                rtp_timestamp: 0,
                packet_count: 1,
                octet_count: 160,
                reports: vec![],
            })
            .encode(),
        );
        bytes.put_slice(
            &Rtcp::Other {
                packet_type: SOURCE_DESCRIPTION,
                payload: Bytes::from_static(&[0, 0, 0, 0]),
            }
            .encode(),
        );

        let decoded = Rtcp::decode_compound(&bytes.freeze()).expect("decodes");
        assert_eq!(decoded.len(), 2);
        assert!(matches!(decoded[0], Rtcp::Sender(_)));
        assert!(matches!(
            decoded[1],
            Rtcp::Other {
                packet_type: SOURCE_DESCRIPTION,
                ..
            }
        ));
    }

    /// A receiver report arriving alone is legal and must be accepted.
    #[test]
    fn a_lone_receiver_report_is_accepted() {
        let report = Rtcp::Receiver(ReceiverReport {
            ssrc: 7,
            reports: vec![],
        });
        assert_eq!(
            Rtcp::decode_compound(&report.encode())
                .expect("decodes")
                .len(),
            1
        );
    }

    #[test]
    fn a_wrong_version_is_rejected() {
        let mut bytes = BytesMut::from(&[0u8; 8][..]);
        bytes[0] = 0b0100_0000;
        assert!(matches!(
            Rtcp::decode_compound(&bytes.freeze()),
            Err(RtcpError::BadVersion(1))
        ));
    }

    #[test]
    fn a_truncated_packet_is_rejected() {
        let full = Rtcp::Receiver(ReceiverReport {
            ssrc: 1,
            reports: vec![block()],
        })
        .encode();
        let truncated = full.slice(..full.len() - 8);
        assert!(matches!(
            Rtcp::decode_compound(&truncated),
            Err(RtcpError::Truncated)
        ));
    }

    /// The acceptance test for M-6: the numbers a report carries are the ones the receive path
    /// actually saw.
    #[test]
    fn a_receiver_report_counts_the_loss_the_buffer_saw() {
        let mut stats = StreamStats::new(99);

        // Ten packets, of which 3 and 7 never arrive.
        let mut arrival = 0u32;
        for sequence in 1u16..=10 {
            arrival += 160;
            if sequence == 3 || sequence == 7 {
                continue;
            }
            stats.on_packet(sequence, u32::from(sequence) * 160, arrival);
        }

        assert_eq!(stats.expected(), 10, "sequence 1 through 10");
        assert_eq!(stats.cumulative_lost(), 2, "two never arrived");

        let block = stats.report_block();
        assert_eq!(block.cumulative_lost, 2);
        assert_eq!(block.ssrc, 99);
        // Two lost out of ten expected is a fifth, which in 256ths is 51.
        assert_eq!(block.fraction_lost, 51, "loss as a fraction in 256ths");
    }

    /// The fraction is loss *since the last report*. A call that lost heavily at the start and
    /// is now clean must report clean, or nobody can see it recover.
    #[test]
    fn the_fraction_covers_the_interval_not_the_whole_call() {
        let mut stats = StreamStats::new(1);
        let mut arrival = 0u32;

        // A bad first interval: half the packets lost. It ends on a packet that *did* arrive,
        // so the interval boundary is not itself a gap — a loss straddling the boundary is
        // attributed to the interval in which it became known, which is correct and would
        // muddy what this test is about.
        for sequence in 1u16..=9 {
            arrival += 160;
            if sequence % 2 == 0 {
                continue;
            }
            stats.on_packet(sequence, u32::from(sequence) * 160, arrival);
        }
        let first = stats.report_block();
        assert!(
            first.fraction_lost > 100,
            "half lost: {}",
            first.fraction_lost
        );

        // A clean second interval.
        for sequence in 10u16..=20 {
            arrival += 160;
            stats.on_packet(sequence, u32::from(sequence) * 160, arrival);
        }
        let second = stats.report_block();
        assert_eq!(second.fraction_lost, 0, "the interval was clean");
        assert_eq!(
            second.cumulative_lost, 4,
            "but the cumulative count still remembers the four lost earlier"
        );
    }

    /// Duplicates can make cumulative loss negative, which is why the field is signed.
    #[test]
    fn duplicates_can_drive_cumulative_loss_negative() {
        let mut stats = StreamStats::new(1);
        let mut arrival = 0u32;
        for sequence in 1u16..=5 {
            arrival += 160;
            stats.on_packet(sequence, u32::from(sequence) * 160, arrival);
            // Every packet arrives twice.
            stats.on_packet(sequence, u32::from(sequence) * 160, arrival);
        }
        assert_eq!(stats.expected(), 5);
        assert_eq!(
            stats.cumulative_lost(),
            -5,
            "ten arrived where five were due"
        );
    }

    /// Evenly spaced packets have no jitter at all — a constant offset between the two clocks
    /// cancels, which is the property that makes the estimate meaningful.
    #[test]
    fn perfectly_spaced_packets_report_no_jitter() {
        let mut stats = StreamStats::new(1);
        for sequence in 1u16..=50 {
            let timestamp = u32::from(sequence) * 160;
            // Arrival offset by a large constant: it must not appear in the estimate.
            stats.on_packet(sequence, timestamp, timestamp + 100_000);
        }
        assert_eq!(stats.jitter(), 0, "even spacing is zero jitter");
    }

    /// And uneven spacing produces a positive estimate that grows with the unevenness.
    #[test]
    fn uneven_arrival_produces_jitter() {
        let mut jittery = StreamStats::new(1);
        let mut arrival = 0u32;
        for sequence in 1u16..=50 {
            // Alternating early and late by 80 timestamp units.
            arrival += if sequence % 2 == 0 { 240 } else { 80 };
            jittery.on_packet(sequence, u32::from(sequence) * 160, arrival);
        }
        assert!(jittery.jitter() > 0, "uneven arrival must show as jitter");

        let mut worse = StreamStats::new(1);
        let mut arrival = 0u32;
        for sequence in 1u16..=50 {
            arrival += if sequence % 2 == 0 { 480 } else { 20 };
            worse.on_packet(sequence, u32::from(sequence) * 160, arrival);
        }
        assert!(
            worse.jitter() > jittery.jitter(),
            "more unevenness, more jitter: {} vs {}",
            worse.jitter(),
            jittery.jitter()
        );
    }

    /// The sequence wrap must be counted, or the extended number goes backwards and the loss
    /// calculation reports eight million packets lost.
    #[test]
    fn the_extended_sequence_number_counts_wraps() {
        let mut stats = StreamStats::new(1);
        let mut arrival = 0u32;
        for sequence in [65_534u16, 65_535, 0, 1, 2] {
            arrival += 160;
            stats.on_packet(sequence, arrival, arrival);
        }
        assert_eq!(
            stats.extended_highest_sequence(),
            0x0001_0002,
            "one wrap, then sequence 2"
        );
        assert_eq!(
            stats.cumulative_lost(),
            0,
            "nothing was lost across the wrap"
        );
    }
}
