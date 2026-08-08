//! RTP packets (RFC 3550 §5.1).
//!
//! The header is twelve fixed bytes plus optional contributing sources and an optional
//! extension, and the whole of it is bit-packed. Every field here has been the cause of a
//! decoder reading someone else's audio as its own, so each is parsed explicitly rather than
//! by casting a struct over the buffer.

use bytes::{BufMut, Bytes, BytesMut};

/// The fixed header size, before CSRCs or extensions.
pub const HEADER_LEN: usize = 12;

/// What can go wrong reading a packet.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RtpError {
    /// Fewer bytes than a header.
    #[error("packet is {0} bytes; an RTP header is {HEADER_LEN}")]
    TooShort(usize),
    /// A version other than 2.
    #[error("RTP version {0}; only version 2 exists")]
    BadVersion(u8),
    /// The header claims more content than the packet holds.
    #[error("header claims more bytes than the packet contains")]
    Truncated,
    /// The padding length is impossible.
    #[error("padding of {0} bytes does not fit")]
    BadPadding(usize),
}

/// An RTP packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    /// Whether this packet marks a significant event — the start of a talkspurt, or the end of
    /// a DTMF tone.
    pub marker: bool,
    /// Which codec the payload is in.
    pub payload_type: u8,
    /// Increases by one per packet, and wraps.
    pub sequence: u16,
    /// The sampling instant of the first byte of payload.
    pub timestamp: u32,
    /// Who sent it.
    pub ssrc: u32,
    /// Sources that contributed, when a mixer combined streams.
    pub csrc: Vec<u32>,
    /// The media.
    pub payload: Bytes,
}

impl Packet {
    /// A packet carrying a payload.
    #[must_use]
    pub fn new(payload_type: u8, sequence: u16, timestamp: u32, ssrc: u32, payload: Bytes) -> Self {
        Self {
            marker: false,
            payload_type,
            sequence,
            timestamp,
            ssrc,
            csrc: Vec::new(),
            payload,
        }
    }

    /// Serialize to the wire.
    #[must_use]
    pub fn encode(&self) -> Bytes {
        let csrc_count = self.csrc.len().min(15);
        let mut out = BytesMut::with_capacity(HEADER_LEN + csrc_count * 4 + self.payload.len());

        // Version 2, no padding, no extension, and the CSRC count in the low nibble.
        let first = 0b1000_0000 | u8::try_from(csrc_count).unwrap_or(0);
        out.put_u8(first);
        out.put_u8((u8::from(self.marker) << 7) | (self.payload_type & 0x7F));
        out.put_u16(self.sequence);
        out.put_u32(self.timestamp);
        out.put_u32(self.ssrc);
        for csrc in self.csrc.iter().take(csrc_count) {
            out.put_u32(*csrc);
        }
        out.put_slice(&self.payload);
        out.freeze()
    }

    /// Parse a packet.
    ///
    /// Rejects rather than guesses. A decoder that reads a malformed packet optimistically
    /// ends up playing header bytes as audio, which is heard as a loud click.
    pub fn decode(bytes: &Bytes) -> Result<Self, RtpError> {
        if bytes.len() < HEADER_LEN {
            return Err(RtpError::TooShort(bytes.len()));
        }

        let first = bytes.first().copied().unwrap_or(0);
        let version = first >> 6;
        if version != 2 {
            return Err(RtpError::BadVersion(version));
        }
        let has_padding = first & 0b0010_0000 != 0;
        let has_extension = first & 0b0001_0000 != 0;
        let csrc_count = usize::from(first & 0x0F);

        let second = bytes.get(1).copied().unwrap_or(0);
        let marker = second & 0x80 != 0;
        let payload_type = second & 0x7F;

        let sequence = u16::from_be_bytes([
            bytes.get(2).copied().unwrap_or(0),
            bytes.get(3).copied().unwrap_or(0),
        ]);
        let timestamp = read_u32(bytes, 4)?;
        let ssrc = read_u32(bytes, 8)?;

        let mut offset = HEADER_LEN;
        let mut csrc = Vec::with_capacity(csrc_count);
        for _ in 0..csrc_count {
            csrc.push(read_u32(bytes, offset)?);
            offset += 4;
        }

        if has_extension {
            // The extension is a 16-bit profile field, a 16-bit length in 32-bit words, then
            // that many words. The length excludes the four bytes of the header itself, which
            // is the detail that makes off-by-one errors here so easy.
            let words = usize::from(u16::from_be_bytes([
                bytes.get(offset + 2).copied().ok_or(RtpError::Truncated)?,
                bytes.get(offset + 3).copied().ok_or(RtpError::Truncated)?,
            ]));
            offset = offset
                .checked_add(4 + words * 4)
                .ok_or(RtpError::Truncated)?;
        }

        if offset > bytes.len() {
            return Err(RtpError::Truncated);
        }
        let mut end = bytes.len();

        if has_padding {
            // The last byte says how many bytes of padding there are, *including itself*.
            let pad = usize::from(bytes.last().copied().unwrap_or(0));
            if pad == 0 || pad > end - offset {
                return Err(RtpError::BadPadding(pad));
            }
            end -= pad;
        }

        Ok(Self {
            marker,
            payload_type,
            sequence,
            timestamp,
            ssrc,
            csrc,
            payload: bytes.slice(offset..end),
        })
    }
}

fn read_u32(bytes: &Bytes, at: usize) -> Result<u32, RtpError> {
    Ok(u32::from_be_bytes([
        bytes.get(at).copied().ok_or(RtpError::Truncated)?,
        bytes.get(at + 1).copied().ok_or(RtpError::Truncated)?,
        bytes.get(at + 2).copied().ok_or(RtpError::Truncated)?,
        bytes.get(at + 3).copied().ok_or(RtpError::Truncated)?,
    ]))
}

/// Compare two sequence numbers across the 16-bit wrap.
///
/// The counter wraps every ~22 minutes at 50 packets per second, so this is an ordinary event
/// in any call worth having, not an edge case. Comparing with `<` instead treats the wrap as a
/// 65535-packet jump backwards and throws away a minute of audio while the buffer resyncs.
#[must_use]
pub fn sequence_is_newer(candidate: u16, current: u16) -> bool {
    // RFC 1982 serial number arithmetic: the difference, read as signed, is the distance.
    candidate != current && candidate.wrapping_sub(current) < 0x8000
}

/// The forward distance from one sequence number to another, across the wrap.
#[must_use]
pub fn sequence_distance(from: u16, to: u16) -> u16 {
    to.wrapping_sub(from)
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

    fn sample_packet() -> Packet {
        Packet::new(
            0,
            1000,
            160_000,
            0xDEAD_BEEF,
            Bytes::from_static(&[0xFF; 160]),
        )
    }

    #[test]
    fn a_packet_round_trips_through_the_wire_format() {
        let original = sample_packet();
        let decoded = Packet::decode(&original.encode()).expect("decodes");
        assert_eq!(decoded, original);
    }

    #[test]
    fn the_header_is_twelve_bytes_before_the_payload() {
        let encoded = sample_packet().encode();
        assert_eq!(encoded.len(), HEADER_LEN + 160);
        assert_eq!(encoded[0] >> 6, 2, "version 2");
        assert_eq!(encoded[1] & 0x7F, 0, "payload type 0");
        assert_eq!(u16::from_be_bytes([encoded[2], encoded[3]]), 1000);
    }

    #[test]
    fn the_marker_bit_survives_a_round_trip() {
        let mut packet = sample_packet();
        packet.marker = true;
        let decoded = Packet::decode(&packet.encode()).expect("decodes");
        assert!(decoded.marker);
        assert_eq!(
            decoded.payload_type, 0,
            "the marker must not bleed into the type"
        );
    }

    /// A payload type of 127 sets every bit the marker does not. Packing them into one byte is
    /// where a decoder starts reading type 127 as a marker with type 0.
    #[test]
    fn a_high_payload_type_does_not_collide_with_the_marker() {
        let mut packet = sample_packet();
        packet.payload_type = 127;
        packet.marker = false;
        let decoded = Packet::decode(&packet.encode()).expect("decodes");
        assert_eq!(decoded.payload_type, 127);
        assert!(!decoded.marker);
    }

    #[test]
    fn contributing_sources_survive() {
        let mut packet = sample_packet();
        packet.csrc = vec![1, 2, 3];
        let decoded = Packet::decode(&packet.encode()).expect("decodes");
        assert_eq!(decoded.csrc, vec![1, 2, 3]);
        assert_eq!(
            decoded.payload, packet.payload,
            "the payload starts after them"
        );
    }

    /// The padding count includes itself and must be removed from the payload. Leaving it in
    /// plays padding as audio.
    #[test]
    fn padding_is_stripped_from_the_payload() {
        let mut raw = BytesMut::new();
        raw.put_u8(0b1010_0000); // version 2, padding set
        raw.put_u8(0);
        raw.put_u16(1);
        raw.put_u32(0);
        raw.put_u32(0);
        raw.put_slice(&[1, 2, 3, 4]);
        raw.put_slice(&[0, 0, 0, 4]); // four bytes of padding, the last being the count

        let decoded = Packet::decode(&raw.freeze()).expect("decodes");
        assert_eq!(decoded.payload.as_ref(), &[1, 2, 3, 4]);
    }

    #[test]
    fn impossible_padding_is_rejected() {
        let mut raw = BytesMut::new();
        raw.put_u8(0b1010_0000);
        raw.put_u8(0);
        raw.put_u16(1);
        raw.put_u32(0);
        raw.put_u32(0);
        raw.put_slice(&[1, 2, 200]); // claims 200 bytes of padding in a 3-byte payload
        assert!(matches!(
            Packet::decode(&raw.freeze()),
            Err(RtpError::BadPadding(200))
        ));
    }

    /// The extension length counts 32-bit words and excludes its own four-byte header, which
    /// is where off-by-one errors here come from.
    #[test]
    fn a_header_extension_is_skipped_not_played() {
        let mut raw = BytesMut::new();
        raw.put_u8(0b1001_0000); // version 2, extension set
        raw.put_u8(0);
        raw.put_u16(7);
        raw.put_u32(0);
        raw.put_u32(0);
        raw.put_u16(0xBEDE); // profile
        raw.put_u16(2); // two words follow
        raw.put_slice(&[9; 8]);
        raw.put_slice(&[1, 2, 3]);

        let decoded = Packet::decode(&raw.freeze()).expect("decodes");
        assert_eq!(
            decoded.payload.as_ref(),
            &[1, 2, 3],
            "the extension is not payload"
        );
    }

    #[test]
    fn a_short_packet_is_rejected() {
        assert!(matches!(
            Packet::decode(&Bytes::from_static(&[0x80, 0, 0])),
            Err(RtpError::TooShort(3))
        ));
    }

    /// Version 1 does not exist in the wild and version 0 is usually a stray STUN packet on the
    /// RTP port. Either way it is not audio.
    #[test]
    fn a_wrong_version_is_rejected() {
        let mut raw = BytesMut::from(&[0u8; 12][..]);
        raw[0] = 0b0100_0000; // version 1
        assert!(matches!(
            Packet::decode(&raw.freeze()),
            Err(RtpError::BadVersion(1))
        ));
    }

    #[test]
    fn a_truncated_csrc_list_is_rejected() {
        let mut raw = BytesMut::from(&[0u8; 12][..]);
        raw[0] = 0b1000_0011; // claims three CSRCs that are not there
        assert!(matches!(
            Packet::decode(&raw.freeze()),
            Err(RtpError::Truncated)
        ));
    }

    /// The failing-first test for this story. The counter wraps every ~22 minutes at 50 packets
    /// per second; treating that as a jump backwards throws away audio while the buffer
    /// resynchronises.
    #[test]
    fn sequence_wraparound_is_ordered_correctly() {
        assert!(sequence_is_newer(1, 0));
        assert!(!sequence_is_newer(0, 1));

        // Across the wrap: 0 follows 65535.
        assert!(sequence_is_newer(0, 65_535));
        assert!(!sequence_is_newer(65_535, 0));
        assert!(sequence_is_newer(5, 65_530));
        assert!(!sequence_is_newer(65_530, 5));

        // A number is never newer than itself.
        assert!(!sequence_is_newer(42, 42));

        // Half the space away is the boundary where "newer" stops meaning anything; the
        // convention is that it counts as older.
        assert!(!sequence_is_newer(0x8000, 0));
        assert!(sequence_is_newer(0x7FFF, 0));
    }

    #[test]
    fn sequence_distance_counts_forward_across_the_wrap() {
        assert_eq!(sequence_distance(0, 1), 1);
        assert_eq!(sequence_distance(65_535, 0), 1);
        assert_eq!(sequence_distance(65_530, 5), 11);
        assert_eq!(sequence_distance(10, 10), 0);
    }
}
