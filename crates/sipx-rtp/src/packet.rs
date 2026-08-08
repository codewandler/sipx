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
///
/// # Stability
///
/// `#[non_exhaustive]`: build one with [`Packet::new`] and assign whatever else it needs. The
/// fields stay `pub` and stay readable and assignable from anywhere — the attribute forbids the
/// struct literal and nothing else, which is why [`Packet::new`] taking five of the eight fields
/// costs a caller nothing.
///
/// The reason is a rate and not a prediction. This type mirrors a wire format that grows, and it
/// has already grown twice under callers: `M-75` added [`Packet::extension`] here and `M-79` added
/// the same field to `sipx_media::Encoded`, and each of those additive changes broke every struct
/// literal naming the type. `M-80` marked the two **together**, because they are the two ends of
/// one relay path and marking one would have left the other free to break the same caller in the
/// same way for the same reason.
///
/// The timing is the rest of the argument. `#[non_exhaustive]` can be **removed** in any minor
/// release without breaking a caller, and can only be **added** in a major one. Marking now keeps
/// both answers reachable; shipping `1.0.0` unmarked would have spent the choice on the answer
/// that two stories had already shown to be wrong. See `docs/roadmap.md`'s v1 predicate 4, which
/// is where a contract stops being editable to fit a change.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
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
    /// The header extension exactly as it arrived — profile field, length word and payload —
    /// or `None` when the packet carried none.
    ///
    /// Kept verbatim so a path that forwards a packet does not silently strip information the
    /// far end relied on. This crate never interprets it: RFC 8285's one- and two-byte forms are
    /// a profile's business, and inventing a reading here would be a guess with a wire effect.
    ///
    /// **The one thing [`Packet::encode`] does read is the length word**, because that word is
    /// where the payload begins. A value here that fails
    /// [`extension_is_self_consistent`] is left off the packet with the X bit
    /// clear, and the payload goes out whole (`M-85`). Anything [`Packet::decode`] produced
    /// passes; a value built by hand is the caller's to get right.
    pub extension: Option<Bytes>,
    /// The media.
    pub payload: Bytes,
}

impl Packet {
    /// A packet carrying a payload.
    #[must_use]
    pub fn new(payload_type: u8, sequence: u16, timestamp: u32, ssrc: u32, payload: Bytes) -> Self {
        Self {
            extension: None,
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
        // Only an extension that agrees with its own length word is written (`M-85`). This is the
        // four-octet filter that used to stand here, generalised: too short to hold a length word
        // was only the case where the disagreement is total. The bytes are dropped rather than the
        // packet because the payload boundary is not in doubt — `payload` says where the media is
        // — so the media has no reason to be lost with the metadata.
        let extension = self
            .extension
            .as_ref()
            .filter(|bytes| extension_is_self_consistent(bytes));
        let extension_len = extension.map_or(0, Bytes::len);
        let mut out = BytesMut::with_capacity(
            HEADER_LEN + csrc_count * 4 + extension_len + self.payload.len(),
        );

        // Version 2, no padding, the extension bit set only when one is actually carried, and the
        // CSRC count in the low nibble. Setting the bit without the bytes would make the next
        // reader consume payload as an extension header.
        let first = 0b1000_0000
            | (u8::from(extension.is_some()) << 4)
            | u8::try_from(csrc_count).unwrap_or(0);
        out.put_u8(first);
        out.put_u8((u8::from(self.marker) << 7) | (self.payload_type & 0x7F));
        out.put_u16(self.sequence);
        out.put_u32(self.timestamp);
        out.put_u32(self.ssrc);
        for csrc in self.csrc.iter().take(csrc_count) {
            out.put_u32(*csrc);
        }
        if let Some(bytes) = extension {
            out.put_slice(bytes);
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

        let mut extension = None;
        if has_extension {
            // The extension is a 16-bit profile field, a 16-bit length in 32-bit words, then
            // that many words. The length excludes the four bytes of the header itself, which
            // is the detail that makes off-by-one errors here so easy.
            let words = usize::from(u16::from_be_bytes([
                bytes.get(offset + 2).copied().ok_or(RtpError::Truncated)?,
                bytes.get(offset + 3).copied().ok_or(RtpError::Truncated)?,
            ]));
            let start = offset;
            offset = offset
                .checked_add(4 + words * 4)
                .ok_or(RtpError::Truncated)?;
            if offset > bytes.len() {
                return Err(RtpError::Truncated);
            }
            // Sliced, not copied, and kept whole. Dropping it here is what made a forwarding path
            // silently strip extensions the far end had asked for (`M-75`).
            extension = Some(bytes.slice(start..offset));
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
            extension,
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

/// Whether a header extension's length word agrees with the bytes that follow it
/// (RFC 3550 §5.3.1).
///
/// The shape is a two-octet profile field, a two-octet length counting 32-bit words, then exactly
/// that many words — so `bytes.len() == 4 + words * 4`, and nothing else. **This is a property of
/// the extension alone**, not of any packet: an extension that passes here can be written onto any
/// packet, and one that fails cannot be written onto any packet at all.
///
/// Both directions of disagreement move the payload boundary rather than the extension's, which is
/// why neither is tolerated. A length word that overstates makes every reader take media as header
/// — on an encrypted leg, media a counter-mode transform then leaves unencrypted and an AEAD
/// transform sends as Associated Data (RFC 7714 §8.2). One that understates makes readers take
/// header as media and play it.
///
/// **What it does not promise.** Nothing about the *content* of those words. RFC 8285's one- and
/// two-byte element forms, and every other profile's, are the profile's business and this crate
/// interprets none of it, so an extension whose elements are nonsense passes here as long as its
/// length word is right. It is not a validity check; it is the question
/// [`Packet::encode`] has to answer before it can write the bytes.
///
/// Anything [`Packet::decode`] produced passes, because decode slices exactly `4 + words * 4`
/// octets and refuses a length word that runs past the datagram. A caller building an extension by
/// hand is the only way to reach a `false`.
#[must_use]
pub fn extension_is_self_consistent(bytes: &[u8]) -> bool {
    let (Some(high), Some(low)) = (bytes.get(2).copied(), bytes.get(3).copied()) else {
        // Under four octets there is no length word to agree with anything.
        return false;
    };
    let words = usize::from(u16::from_be_bytes([high, low]));
    // A 16-bit word count is at most 262 140 octets, so this cannot overflow.
    bytes.len() == 4 + words * 4
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

    #[test]
    fn the_constructor_reaches_every_field_a_literal_could_set() {
        // `M-80`. `Packet` is `#[non_exhaustive]`, so the literal below is the one thing a
        // downstream crate may no longer write. What it may write is `new` plus assignments, and
        // the attribute is only honest if the two reach the same values: an attribute that also
        // removed reachable states would be a functional change wearing a compatibility argument.
        //
        // This is also the pressure that keeps it true. A field added later stops this literal
        // compiling, so whoever adds it has to name it here — and can only make the assertion
        // pass again by making the field reachable from `new` or from an assignment.
        let mut built = Packet::new(96, 7, 1234, 0xDEAD_BEEF, Bytes::from_static(&[1, 2, 3]));
        built.marker = true;
        built.csrc = vec![0xAAAA, 0xBBBB];
        built.extension = Some(Bytes::from_static(&[0xBE, 0xDE, 0, 0]));

        let literal = Packet {
            marker: true,
            payload_type: 96,
            sequence: 7,
            timestamp: 1234,
            ssrc: 0xDEAD_BEEF,
            csrc: vec![0xAAAA, 0xBBBB],
            extension: Some(Bytes::from_static(&[0xBE, 0xDE, 0, 0])),
            payload: Bytes::from_static(&[1, 2, 3]),
        };

        assert_eq!(literal, built);
    }
}
