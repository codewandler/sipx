//! DTMF as named telephone events (RFC 4733).
//!
//! A keypress is not audio. Sending it as audio works over a clean codec and falls apart the
//! moment anything transcodes, so RFC 4733 carries the *digit* instead, in a four-byte
//! payload on its own payload type.
//!
//! ```text
//!  0                   1                   2                   3
//!  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! |     event     |E|R| volume    |          duration             |
//! +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//! ```
//!
//! The part that is easy to get wrong is not the layout, it is the timing. One keypress is a
//! *run* of packets that all share the RTP timestamp of the moment the tone started; the
//! duration field grows while the digit is held. A sender that advances the timestamp per
//! packet turns one keypress into a stream of separate digits, and the far end dials
//! something nobody typed.

use bytes::{BufMut, Bytes, BytesMut};

/// How many bytes a telephone event occupies.
pub const EVENT_LEN: usize = 4;

/// The conventional payload type for `telephone-event`. Dynamic, so the SDP decides — this is
/// only the value sipx offers.
pub const DEFAULT_PAYLOAD_TYPE: u8 = 101;

/// A DTMF digit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Digit {
    /// `0`–`9`.
    Number(u8),
    /// `*`.
    Star,
    /// `#`.
    Hash,
    /// `A`–`D`, the fourth column that most keypads do not have.
    Letter(u8),
}

impl Digit {
    /// The RFC 4733 event code.
    #[must_use]
    pub fn code(self) -> u8 {
        match self {
            Self::Number(n) => n.min(9),
            Self::Star => 10,
            Self::Hash => 11,
            Self::Letter(l) => 12 + l.min(3),
        }
    }

    /// The digit an event code names, if it is one.
    #[must_use]
    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            0..=9 => Some(Self::Number(code)),
            10 => Some(Self::Star),
            11 => Some(Self::Hash),
            12..=15 => Some(Self::Letter(code - 12)),
            // 16 is flash and above that are other signals; sipx carries DTMF only, and
            // reporting a flash as a digit would be worse than not reporting it.
            _ => None,
        }
    }

    /// The digit a character names.
    #[must_use]
    pub fn from_char(c: char) -> Option<Self> {
        match c {
            '0'..='9' => u8::try_from(u32::from(c) - u32::from('0'))
                .ok()
                .map(Self::Number),
            '*' => Some(Self::Star),
            '#' => Some(Self::Hash),
            'A'..='D' => u8::try_from(u32::from(c) - u32::from('A'))
                .ok()
                .map(Self::Letter),
            'a'..='d' => u8::try_from(u32::from(c) - u32::from('a'))
                .ok()
                .map(Self::Letter),
            _ => None,
        }
    }

    /// How the digit is written.
    #[must_use]
    pub fn as_char(self) -> char {
        match self {
            Self::Number(n) => char::from(b'0' + n.min(9)),
            Self::Star => '*',
            Self::Hash => '#',
            Self::Letter(l) => char::from(b'A' + l.min(3)),
        }
    }
}

impl std::fmt::Display for Digit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_char())
    }
}

/// One telephone-event payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    /// Which digit.
    pub digit: Digit,
    /// Whether this packet ends the tone.
    pub end: bool,
    /// Power, in -dBm0. Zero is loudest; RFC 4733 §2.3.2 recommends no louder than -3.
    pub volume: u8,
    /// How long the tone has lasted so far, in timestamp units.
    pub duration: u16,
}

impl Event {
    /// An event for a digit that is still sounding.
    #[must_use]
    pub fn new(digit: Digit, duration: u16) -> Self {
        Self {
            digit,
            end: false,
            volume: 10,
            duration,
        }
    }

    /// Serialize to the four-byte payload.
    #[must_use]
    pub fn encode(&self) -> Bytes {
        let mut out = BytesMut::with_capacity(EVENT_LEN);
        out.put_u8(self.digit.code());
        // The reserved bit stays zero. Volume is six bits, so a louder-than-representable
        // value has to be clamped rather than allowed to overflow into the reserved bit.
        out.put_u8((u8::from(self.end) << 7) | (self.volume & 0x3F));
        out.put_u16(self.duration);
        out.freeze()
    }

    /// Read a four-byte payload.
    ///
    /// Returns `None` for anything that is not a telephone event this crate understands —
    /// including event codes above 15, which are signals rather than digits.
    #[must_use]
    pub fn decode(payload: &[u8]) -> Option<Self> {
        if payload.len() < EVENT_LEN {
            return None;
        }
        let digit = Digit::from_code(*payload.first()?)?;
        let second = *payload.get(1)?;
        Some(Self {
            digit,
            end: second & 0x80 != 0,
            volume: second & 0x3F,
            duration: u16::from_be_bytes([*payload.get(2)?, *payload.get(3)?]),
        })
    }
}

/// How many copies of the final packet to send.
///
/// RFC 4733 §2.5.1.3: the end of a tone is retransmitted so that losing one packet does not
/// leave the far end holding a digit down forever. Three is the RFC's number.
pub const END_RETRANSMISSIONS: usize = 3;

/// One packet of a keypress: the event payload, plus where its segment sits in the
/// event's timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TonePacket {
    /// The four-byte payload.
    pub event: Event,
    /// The segment's start, in timestamp units after the event began.
    ///
    /// Zero for any event short enough to fit one segment. RFC 4733 §2.5.1.3: an event
    /// that outlives the 16-bit duration field is continued as a new segment with a fresh
    /// RTP timestamp, and this offset is what that timestamp moves by.
    pub segment_offset: u32,
}

/// Build the packets for one keypress.
///
/// Every packet of a segment shares one RTP timestamp — the segment's start — which is
/// what marks them as one tone. The duration grows across the run, and the last packet is
/// repeated with the end bit set. An event too long for the duration field continues as a
/// new segment (RFC 4733 §2.5.1.3): the duration restarts, the offset advances, and only
/// the final segment carries the end bit.
#[must_use]
pub fn tone(digit: Digit, packets: usize, samples_per_packet: u16) -> Vec<TonePacket> {
    let steps = packets.max(1);
    let mut events = Vec::with_capacity(steps + END_RETRANSMISSIONS);
    let mut segment_offset: u32 = 0;
    let mut duration: u32 = 0;

    for _ in 0..steps {
        if duration + u32::from(samples_per_packet) > u32::from(u16::MAX) {
            // The field is full: the event continues as a new segment rather than
            // saturating, which would report a key stuck at 65535 for as long as it is
            // held (RFC 4733 §2.5.1.3). The segment before it ends without the end bit —
            // that bit ends the *event*, and only the last segment carries it.
            segment_offset += duration;
            duration = 0;
        }
        duration += u32::from(samples_per_packet);
        events.push(TonePacket {
            event: Event::new(digit, u16::try_from(duration).unwrap_or(u16::MAX)),
            segment_offset,
        });
    }

    for _ in 0..END_RETRANSMISSIONS {
        events.push(TonePacket {
            event: Event {
                digit,
                end: true,
                volume: 10,
                duration: u16::try_from(duration).unwrap_or(u16::MAX),
            },
            segment_offset,
        });
    }
    events
}

/// One telephone event completed by the receive state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Completed {
    /// Which key was pressed.
    pub digit: Digit,
    /// How long it was held, in the negotiated RTP clock's timestamp units.
    ///
    /// This is wider than the wire's 16-bit duration because a long event can span several
    /// timestamp segments (RFC 4733 §2.5.2.3).
    pub duration: u32,
}

#[derive(Debug, Clone, Copy)]
struct Current {
    digit: Digit,
    started_at: u32,
    segment_at: u32,
    duration: u32,
}

#[derive(Debug, Clone, Copy)]
struct Reported {
    digit: Digit,
    segment_at: u32,
}

/// Reassembles received events into digits.
///
/// The whole job is reporting each keypress exactly once. A tone arrives as many packets and
/// its end arrives three times, so a receiver that reports what it receives reports every
/// digit four or more times — which, for a caller entering a PIN, is a wrong PIN.
///
/// This type reads no clock. [`Self::timeout`] is the explicit fired-timer input supplied by the
/// media worker after RFC 4733 §2.5.2.2's bounded silence interval.
#[derive(Debug, Default)]
pub struct Receiver {
    current: Option<Current>,
    /// The most recently completed identity, so final-report retransmissions cannot emit twice.
    reported: Option<Reported>,
    /// RTP ordering includes ordinary audio packets too; [`Self::observe_non_event`] advances it
    /// while no event payload is being decoded.
    last_sequence: Option<u16>,
}

impl Receiver {
    /// A receiver with nothing in progress.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one telephone-event packet.
    ///
    /// The two slots are allocation-free and preserve order in the one case that can complete two
    /// events at once: a new marked event replaces a tone whose end reports were all lost, and the
    /// first received report for the new event already has its E bit set. Empty slots mean no
    /// application event.
    pub fn push(
        &mut self,
        sequence: u16,
        timestamp: u32,
        marker: bool,
        event: &Event,
    ) -> [Option<Completed>; 2] {
        if !self.advance(sequence) {
            return [None, None];
        }

        // A report for the same event code and segment timestamp after completion is an update or
        // final-report retransmission, never a reason to restart the key. A different timestamp
        // is independent even if its first marked packet was lost, which is how "44" remains two
        // digits on a lossy path.
        if self.current.is_none()
            && self.reported.is_some_and(|reported| {
                reported.digit == event.digit && reported.segment_at == timestamp
            })
        {
            return [None, None];
        }

        let mut prior = None;
        match self.current {
            None => self.start(timestamp, *event),
            Some(current) if marker || current.digit != event.digit => {
                prior = self.complete();
                self.start(timestamp, *event);
            }
            Some(current) => {
                let duration = if current.segment_at == timestamp {
                    current.duration.max(u32::from(event.duration))
                } else {
                    // Only the first segment carries M (RFC 4733 §2.2.2). Timestamp distance
                    // therefore turns the final segment's 16-bit field into the full event
                    // duration without reading a wall clock.
                    timestamp
                        .wrapping_sub(current.started_at)
                        .saturating_add(u32::from(event.duration))
                        .max(current.duration)
                };
                self.current = Some(Current {
                    segment_at: timestamp,
                    duration,
                    ..current
                });
            }
        }

        if !event.end {
            return [prior, None];
        }
        match prior {
            Some(prior) => [Some(prior), self.complete()],
            None => [self.complete(), None],
        }
    }

    /// Advance RTP ordering for an accepted packet that is not a telephone event.
    ///
    /// Audio shares the RTP sequence space. Observing it prevents a long pause between keypresses
    /// from exceeding half the serial-number range and making the next digit look older.
    pub fn observe_non_event(&mut self, sequence: u16) {
        let _ = self.advance(sequence);
    }

    /// Finish a current event when ordinary media resumes after all of its end reports were lost.
    pub fn finish_on_media(&mut self, sequence: u16) -> Option<Completed> {
        if !self.advance(sequence) {
            return None;
        }
        self.complete()
    }

    /// Finish a current event when the media worker fires its bounded silence expiration.
    pub fn timeout(&mut self) -> Option<Completed> {
        self.complete()
    }

    /// Discard all receive history at a media-generation boundary.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The digit currently sounding, if any.
    #[must_use]
    pub fn in_progress(&self) -> Option<Digit> {
        self.current.map(|current| current.digit)
    }

    fn advance(&mut self, sequence: u16) -> bool {
        if self
            .last_sequence
            .is_some_and(|last| !crate::packet::sequence_is_newer(sequence, last))
        {
            return false;
        }
        self.last_sequence = Some(sequence);
        true
    }

    fn start(&mut self, timestamp: u32, event: Event) {
        self.current = Some(Current {
            digit: event.digit,
            started_at: timestamp,
            segment_at: timestamp,
            duration: u32::from(event.duration),
        });
    }

    fn complete(&mut self) -> Option<Completed> {
        let current = self.current.take()?;
        self.reported = Some(Reported {
            digit: current.digit,
            segment_at: current.segment_at,
        });
        Some(Completed {
            digit: current.digit,
            duration: current.duration,
        })
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

    #[test]
    fn every_digit_maps_to_its_rfc_event_code() {
        for (c, code) in [
            ('0', 0),
            ('9', 9),
            ('*', 10),
            ('#', 11),
            ('A', 12),
            ('D', 15),
        ] {
            let digit = Digit::from_char(c).expect("a digit");
            assert_eq!(digit.code(), code, "{c}");
            assert_eq!(Digit::from_code(code), Some(digit));
            assert_eq!(digit.as_char(), c);
        }
    }

    #[test]
    fn lowercase_letters_are_accepted_and_normalised() {
        assert_eq!(Digit::from_char('b'), Digit::from_char('B'));
        assert_eq!(Digit::from_char('b').expect("a digit").as_char(), 'B');
    }

    #[test]
    fn a_character_that_is_not_a_digit_is_refused() {
        for c in ['x', ' ', '+', 'E', '\n'] {
            assert!(Digit::from_char(c).is_none(), "{c:?} is not a DTMF digit");
        }
    }

    /// Event code 16 is flash, and above that are other signals. Reporting one as a digit
    /// would be worse than not reporting it: the application would dial something.
    #[test]
    fn event_codes_above_fifteen_are_not_digits() {
        assert!(Digit::from_code(16).is_none(), "16 is flash, not a digit");
        assert!(Digit::from_code(255).is_none());
    }

    #[test]
    fn an_event_round_trips_through_its_payload() {
        let event = Event {
            digit: Digit::Hash,
            end: true,
            volume: 7,
            duration: 1600,
        };
        let decoded = Event::decode(&event.encode()).expect("decodes");
        assert_eq!(decoded, event);
    }

    #[test]
    fn the_payload_is_four_bytes_in_the_rfc_layout() {
        let encoded = Event::new(Digit::Number(5), 320).encode();
        assert_eq!(encoded.len(), EVENT_LEN);
        assert_eq!(encoded[0], 5, "event code");
        assert_eq!(encoded[1] & 0x80, 0, "end bit clear");
        assert_eq!(encoded[1] & 0x3F, 10, "volume");
        assert_eq!(u16::from_be_bytes([encoded[2], encoded[3]]), 320);
    }

    /// The volume field is six bits. A value that does not fit must be clamped, not allowed to
    /// overflow into the end bit — where it would end a tone that is still sounding.
    #[test]
    fn an_oversized_volume_cannot_set_the_end_bit() {
        let event = Event {
            digit: Digit::Number(1),
            end: false,
            volume: 255,
            duration: 160,
        };
        let encoded = event.encode();
        assert_eq!(encoded[1] & 0x80, 0, "the end bit must stay clear");
        let decoded = Event::decode(&encoded).expect("decodes");
        assert!(!decoded.end);
    }

    #[test]
    fn a_short_payload_is_refused() {
        assert!(Event::decode(&[]).is_none());
        assert!(Event::decode(&[5, 0, 1]).is_none());
    }

    /// The duration grows across a tone, and the end is repeated three times so that losing
    /// one packet does not leave the far end holding the digit down.
    #[test]
    fn a_tone_grows_in_duration_and_ends_three_times() {
        let events = tone(Digit::Number(7), 4, 160);
        assert_eq!(events.len(), 4 + END_RETRANSMISSIONS);

        let sounding: Vec<u16> = events
            .iter()
            .filter(|p| !p.event.end)
            .map(|p| p.event.duration)
            .collect();
        assert_eq!(sounding, vec![160, 320, 480, 640], "duration accumulates");

        let ends: Vec<&Event> = events
            .iter()
            .filter(|p| p.event.end)
            .map(|p| &p.event)
            .collect();
        assert_eq!(ends.len(), 3);
        assert!(
            ends.iter().all(|e| e.duration == 640),
            "every end packet reports the full duration"
        );
        assert!(events.iter().all(|p| p.event.digit == Digit::Number(7)));
        assert!(
            events.iter().all(|p| p.segment_offset == 0),
            "a short keypress is one segment"
        );
    }

    /// RFC 4733 §2.5.1.3: an event that outlives the 16-bit duration field MUST be
    /// continued as a new segment — fresh start timestamp, duration restarting — never
    /// saturated at 65535, which reports a stuck key from ~8.19 s onward at 8 kHz.
    #[test]
    fn a_long_event_is_segmented_rather_than_saturated() {
        // Ten seconds at 8 kHz, 160 samples per packet: 500 packets, 80000 units in all —
        // more than one segment can carry.
        let events = tone(Digit::Number(1), 500, 160);
        assert!(
            events
                .iter()
                .all(|packet| packet.event.duration != u16::MAX),
            "no packet may report a saturated duration"
        );

        // 409 packets fill the first segment (65440 units); packet 410 starts the second.
        let second = events
            .iter()
            .find(|packet| packet.segment_offset > 0)
            .expect("the event is too long for one segment");
        assert_eq!(second.segment_offset, 65_440);
        assert_eq!(second.event.duration, 160, "the duration restarts");

        // Only the last segment ends the event; a non-final segment just stops.
        assert!(
            events
                .iter()
                .filter(|packet| packet.event.end)
                .all(|packet| packet.segment_offset == 65_440),
            "the end bit belongs to the event, not to a segment"
        );

        // The segments cover the whole keypress, no more and no less.
        let last = events.last().expect("an end packet");
        assert_eq!(
            last.segment_offset + u32::from(last.event.duration),
            80_000,
            "500 packets of 160 units"
        );
    }

    /// The receiver's whole job: one keypress reported once, however many packets carried it.
    /// A receiver that reports what it receives turns a four-digit PIN into a wrong one.
    #[test]
    fn a_tone_is_reported_exactly_once() {
        let mut receiver = Receiver::new();
        let completed = receive_tone(&mut receiver, 10, 1000, Digit::Number(3), 5);
        assert_eq!(
            completed,
            vec![Completed {
                digit: Digit::Number(3),
                duration: 800,
            }],
            "one keypress, one digit"
        );
    }

    /// Two presses of the same key are two digits, told apart by their timestamps. Without
    /// that, "44" is indistinguishable from one long "4".
    #[test]
    fn the_same_digit_pressed_twice_is_two_digits() {
        let mut receiver = Receiver::new();
        let mut digits = Vec::new();
        for (sequence, timestamp) in [(10, 1000u32), (20, 5000)] {
            digits.extend(
                receive_tone(&mut receiver, sequence, timestamp, Digit::Number(4), 3)
                    .into_iter()
                    .map(|completed| completed.digit),
            );
        }
        assert_eq!(digits, vec![Digit::Number(4), Digit::Number(4)]);
    }

    /// A whole sequence, as an application would see it.
    #[test]
    fn a_sequence_of_digits_arrives_in_order() {
        let mut receiver = Receiver::new();
        let mut collected = String::new();
        for (index, c) in "1234*#".chars().enumerate() {
            let digit = Digit::from_char(c).expect("a digit");
            let timestamp = 1000 + u32::try_from(index).unwrap_or(0) * 2000;
            let sequence = u16::try_from(index * 10).unwrap_or(0);
            for completed in receive_tone(&mut receiver, sequence, timestamp, digit, 3) {
                collected.push(completed.digit.as_char());
            }
        }
        assert_eq!(collected, "1234*#");
    }

    /// Losing packets in the middle of a tone must not lose the digit: the end packet is what
    /// reports it, and there are three of those.
    #[test]
    fn a_digit_survives_losing_all_but_one_end_packet() {
        let mut receiver = Receiver::new();
        let events = tone(Digit::Star, 5, 160);
        // Only the last packet arrives.
        let last = events.last().expect("an end packet");
        assert_eq!(
            receiver.push(10, 2000, false, &last.event),
            [
                Some(Completed {
                    digit: Digit::Star,
                    duration: 800,
                }),
                None,
            ]
        );
    }

    #[test]
    fn the_digit_in_progress_is_visible_before_the_tone_ends() {
        let mut receiver = Receiver::new();
        let events = tone(Digit::Hash, 3, 160);
        assert!(receiver.in_progress().is_none());
        receiver.push(10, 1000, true, &events[0].event);
        assert_eq!(receiver.in_progress(), Some(Digit::Hash));
        for (index, packet) in events[1..].iter().enumerate() {
            receiver.push(
                11 + u16::try_from(index).unwrap_or(0),
                1000,
                false,
                &packet.event,
            );
        }
        assert!(receiver.in_progress().is_none(), "the tone is over");
    }

    fn receive_tone(
        receiver: &mut Receiver,
        first_sequence: u16,
        timestamp: u32,
        digit: Digit,
        packets: usize,
    ) -> Vec<Completed> {
        tone(digit, packets, 160)
            .into_iter()
            .enumerate()
            .flat_map(|(index, packet)| {
                receiver
                    .push(
                        first_sequence.wrapping_add(u16::try_from(index).unwrap_or(0)),
                        timestamp.wrapping_add(packet.segment_offset),
                        index == 0,
                        &packet.event,
                    )
                    .into_iter()
                    .flatten()
            })
            .collect()
    }

    fn event(digit: char, end: bool, duration: u16) -> Event {
        Event {
            digit: Digit::from_char(digit).expect("a digit"),
            end,
            volume: 10,
            duration,
        }
    }

    fn push_wire(receiver: &mut Receiver, bytes: &'static [u8]) -> [Option<Completed>; 2] {
        let packet = crate::packet::Packet::decode(&Bytes::from_static(bytes)).expect("valid RTP");
        let event = Event::decode(&packet.payload).expect("valid telephone event");
        receiver.push(packet.sequence, packet.timestamp, packet.marker, &event)
    }

    /// D5 is byte-pinned so the state proof cannot accidentally test a convenient struct that the
    /// RTP decoder would never produce.
    #[test]
    fn negotiated_payload_96_wire_vector_completes_with_its_duration() {
        let mut receiver = Receiver::new();
        assert_eq!(
            push_wire(
                &mut receiver,
                &[
                    0x80, 0xe0, 0x03, 0xe8, 0x00, 0x00, 0x03, 0xe8, 0xde, 0xca, 0xfb, 0xad, 0x01,
                    0x0a, 0x00, 0xa0,
                ],
            ),
            [None, None]
        );
        assert_eq!(
            push_wire(
                &mut receiver,
                &[
                    0x80, 0x60, 0x03, 0xe9, 0x00, 0x00, 0x03, 0xe8, 0xde, 0xca, 0xfb, 0xad, 0x01,
                    0x0a, 0x01, 0x40,
                ],
            ),
            [None, None]
        );
        assert_eq!(
            push_wire(
                &mut receiver,
                &[
                    0x80, 0x60, 0x03, 0xea, 0x00, 0x00, 0x03, 0xe8, 0xde, 0xca, 0xfb, 0xad, 0x01,
                    0x8a, 0x01, 0xe0,
                ],
            ),
            [
                Some(Completed {
                    digit: Digit::Number(1),
                    duration: 480,
                }),
                None,
            ]
        );
    }

    /// D5/D6: duration grows, the E-bit report completes once, and both its recommended
    /// retransmissions and packets delivered out of sequence are absorbed.
    #[test]
    fn receive_vectors_absorb_reordering_duplicates_and_end_retransmissions() {
        let mut receiver = Receiver::new();
        assert_eq!(
            receiver.push(1000, 1000, true, &event('1', false, 160)),
            [None, None]
        );
        assert_eq!(
            receiver.push(1002, 1000, false, &event('1', false, 480)),
            [None, None]
        );
        assert_eq!(
            receiver.push(1001, 1000, false, &event('1', false, 320)),
            [None, None],
            "the late continuation cannot reduce duration"
        );
        assert_eq!(
            receiver.push(1003, 1000, false, &event('1', true, 480)),
            [
                Some(Completed {
                    digit: Digit::Number(1),
                    duration: 480,
                }),
                None,
            ]
        );
        assert_eq!(
            receiver.push(1004, 1000, false, &event('1', true, 480)),
            [None, None]
        );
        assert_eq!(
            receiver.push(1005, 1000, false, &event('1', true, 480)),
            [None, None]
        );
        assert_eq!(
            receiver.push(1005, 1000, false, &event('1', true, 480)),
            [None, None]
        );
    }

    /// D7: time is an input. The RTP core neither reads a clock nor invents extra duration.
    #[test]
    fn fired_silence_reports_the_greatest_wire_duration_once() {
        let mut receiver = Receiver::new();
        assert_eq!(
            push_wire(
                &mut receiver,
                &[
                    0x80, 0xe0, 0x07, 0xd0, 0x00, 0x00, 0x0b, 0xb8, 0xde, 0xca, 0xfb, 0xad, 0x03,
                    0x0a, 0x00, 0xa0,
                ],
            ),
            [None, None]
        );
        assert_eq!(
            push_wire(
                &mut receiver,
                &[
                    0x80, 0x60, 0x07, 0xd1, 0x00, 0x00, 0x0b, 0xb8, 0xde, 0xca, 0xfb, 0xad, 0x03,
                    0x0a, 0x01, 0x40,
                ],
            ),
            [None, None]
        );
        assert_eq!(
            receiver.timeout(),
            Some(Completed {
                digit: Digit::Number(3),
                duration: 320,
            })
        );
        assert_eq!(receiver.timeout(), None);
        assert_eq!(
            receiver.push(2002, 3000, false, &event('3', true, 320)),
            [None, None]
        );
    }

    /// A new M-bit event closes a prior event whose final reports were all lost, without making
    /// the new event wait behind it or changing their order.
    #[test]
    fn a_marked_event_closes_an_unfinished_predecessor_in_order() {
        let mut receiver = Receiver::new();
        receiver.push(10, 1000, true, &event('1', false, 320));
        assert_eq!(
            receiver.push(11, 1320, true, &event('2', true, 160)),
            [
                Some(Completed {
                    digit: Digit::Number(1),
                    duration: 320,
                }),
                Some(Completed {
                    digit: Digit::Number(2),
                    duration: 160,
                }),
            ]
        );
    }

    #[test]
    fn ordinary_media_closes_a_tone_but_an_unknown_payload_only_advances_ordering() {
        let mut receiver = Receiver::new();
        receiver.push(65_534, 1000, true, &event('8', false, 240));
        receiver.observe_non_event(65_535);
        assert_eq!(receiver.in_progress(), Some(Digit::Number(8)));
        assert_eq!(
            receiver.finish_on_media(0),
            Some(Completed {
                digit: Digit::Number(8),
                duration: 240,
            }),
            "sequence wrap is forward progress"
        );
    }

    /// D8: replacement owns a fresh receiver. Incomplete state and duplicate history from the
    /// retired worker are not inherited by the new media generation.
    #[test]
    fn reset_discards_incomplete_and_reported_state() {
        let mut receiver = Receiver::new();
        receiver.push(10, 1000, true, &event('4', false, 160));
        receiver.reset();
        assert!(receiver.in_progress().is_none());
        assert_eq!(receiver.timeout(), None);

        assert_eq!(
            receiver.push(1, 5000, true, &event('4', true, 320)),
            [
                Some(Completed {
                    digit: Digit::Number(4),
                    duration: 320,
                }),
                None,
            ],
            "the replacement generation has independent ordering and identity"
        );
    }
}
