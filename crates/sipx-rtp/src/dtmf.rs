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

/// Build the packets for one keypress.
///
/// Every packet shares `start_timestamp` — that is what marks them as one tone. The duration
/// grows across the run, and the last packet is repeated with the end bit set.
#[must_use]
pub fn tone(digit: Digit, packets: usize, samples_per_packet: u16) -> Vec<Event> {
    let mut events = Vec::with_capacity(packets + END_RETRANSMISSIONS);
    let steps = packets.max(1);

    for step in 1..=steps {
        let duration = samples_per_packet.saturating_mul(u16::try_from(step).unwrap_or(1));
        events.push(Event::new(digit, duration));
    }

    let total = samples_per_packet.saturating_mul(u16::try_from(steps).unwrap_or(1));
    for _ in 0..END_RETRANSMISSIONS {
        events.push(Event {
            digit,
            end: true,
            volume: 10,
            duration: total,
        });
    }
    events
}

/// Reassembles received events into digits.
///
/// The whole job is reporting each keypress exactly once. A tone arrives as many packets and
/// its end arrives three times, so a receiver that reports what it receives reports every
/// digit four or more times — which, for a caller entering a PIN, is a wrong PIN.
#[derive(Debug, Default)]
pub struct Receiver {
    /// The tone currently sounding, identified by its RTP timestamp.
    current: Option<(u32, Digit)>,
    /// Timestamps already reported, so the end retransmissions are absorbed.
    reported: Option<u32>,
}

impl Receiver {
    /// A receiver with nothing in progress.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one telephone-event packet.
    ///
    /// Returns the digit when the tone ends, and `None` while it is still sounding or if this
    /// packet is a repeat of an end already reported.
    pub fn push(&mut self, timestamp: u32, event: &Event) -> Option<Digit> {
        // The timestamp identifies the tone. A new one is a new keypress even if the digit is
        // the same, which is how "44" is told apart from a single long "4".
        if self.reported == Some(timestamp) {
            return None;
        }

        match self.current {
            Some((ts, _)) if ts == timestamp => {}
            _ => self.current = Some((timestamp, event.digit)),
        }

        if event.end {
            let digit = self.current.take().map(|(_, digit)| digit)?;
            self.reported = Some(timestamp);
            return Some(digit);
        }
        None
    }

    /// The digit currently sounding, if any.
    #[must_use]
    pub fn in_progress(&self) -> Option<Digit> {
        self.current.map(|(_, digit)| digit)
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
            .filter(|e| !e.end)
            .map(|e| e.duration)
            .collect();
        assert_eq!(sounding, vec![160, 320, 480, 640], "duration accumulates");

        let ends: Vec<&Event> = events.iter().filter(|e| e.end).collect();
        assert_eq!(ends.len(), 3);
        assert!(
            ends.iter().all(|e| e.duration == 640),
            "every end packet reports the full duration"
        );
        assert!(events.iter().all(|e| e.digit == Digit::Number(7)));
    }

    /// The receiver's whole job: one keypress reported once, however many packets carried it.
    /// A receiver that reports what it receives turns a four-digit PIN into a wrong one.
    #[test]
    fn a_tone_is_reported_exactly_once() {
        let mut receiver = Receiver::new();
        let mut digits = Vec::new();
        for event in tone(Digit::Number(3), 5, 160) {
            if let Some(digit) = receiver.push(1000, &event) {
                digits.push(digit);
            }
        }
        assert_eq!(digits, vec![Digit::Number(3)], "one keypress, one digit");
    }

    /// Two presses of the same key are two digits, told apart by their timestamps. Without
    /// that, "44" is indistinguishable from one long "4".
    #[test]
    fn the_same_digit_pressed_twice_is_two_digits() {
        let mut receiver = Receiver::new();
        let mut digits = Vec::new();
        for timestamp in [1000u32, 5000] {
            for event in tone(Digit::Number(4), 3, 160) {
                if let Some(digit) = receiver.push(timestamp, &event) {
                    digits.push(digit);
                }
            }
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
            for event in tone(digit, 3, 160) {
                if let Some(reported) = receiver.push(timestamp, &event) {
                    collected.push(reported.as_char());
                }
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
        assert_eq!(receiver.push(2000, last), Some(Digit::Star));
    }

    #[test]
    fn the_digit_in_progress_is_visible_before_the_tone_ends() {
        let mut receiver = Receiver::new();
        let events = tone(Digit::Hash, 3, 160);
        assert!(receiver.in_progress().is_none());
        receiver.push(1000, &events[0]);
        assert_eq!(receiver.in_progress(), Some(Digit::Hash));
        for event in &events[1..] {
            receiver.push(1000, event);
        }
        assert!(receiver.in_progress().is_none(), "the tone is over");
    }
}
