//! A forwarded packet keeps the header extension it arrived with (`M-75`).
//!
//! Before this, `decode` skipped the extension to find the payload and `encode` never wrote one,
//! so any path that parsed a packet and re-encoded it silently stripped information the far end
//! had negotiated. Harmless for the audio paths that author their own packets; a correctness trap
//! for a relay.

#![allow(clippy::expect_used, clippy::indexing_slicing)]

use bytes::Bytes;
use sipx_rtp::packet::Packet;

/// Profile `0xBEDE` (RFC 8285 one-byte form), one 32-bit word of extension data, then payload.
fn with_extension() -> Bytes {
    let mut raw = vec![
        0b1001_0000, // version 2, extension bit set, no CSRCs
        0x00,        // payload type 0, no marker
        0x00,
        0x2A, // sequence
        0x00,
        0x00,
        0x00,
        0x64, // timestamp
        0x00,
        0x00,
        0x00,
        0x07, // ssrc
        0xBE,
        0xDE,
        0x00,
        0x01, // extension profile + one word
        0x10,
        0xAA,
        0x00,
        0x00, // the word itself
    ];
    raw.extend_from_slice(&[0xD5; 8]); // payload
    Bytes::from(raw)
}

#[test]
fn a_re_encoded_packet_carries_the_extension_it_arrived_with() {
    let arrived = with_extension();
    let packet = Packet::decode(&arrived).expect("the fixture decodes");
    assert!(
        packet.extension.is_some(),
        "the extension was dropped at decode, so nothing downstream can preserve it"
    );
    let re_encoded = packet.encode();
    assert_eq!(
        arrived, re_encoded,
        "re-encoding changed the bytes: a forwarding path is stripping or rewriting the header \
         extension the far end negotiated"
    );
}

#[test]
fn a_packet_without_an_extension_does_not_grow_one() {
    let arrived = with_extension();
    let mut packet = Packet::decode(&arrived).expect("the fixture decodes");
    packet.extension = None;
    let re_encoded = packet.encode();
    let first = re_encoded.first().copied().expect("a header byte");
    assert_eq!(
        first & 0b0001_0000,
        0,
        "the extension bit is set with no extension bytes behind it, so the next reader will \
         consume payload as an extension header"
    );
}

#[test]
fn the_payload_survives_an_extension_round_trip() {
    let packet = Packet::decode(&with_extension()).expect("the fixture decodes");
    assert_eq!(
        packet.payload.as_ref(),
        [0xD5; 8].as_slice(),
        "the extension length was misread, so the payload boundary moved"
    );
}

/// `encode` writes no extension whose length word disagrees with the bytes behind it (`M-85`).
///
/// The length word is not a description a reader can ignore: it is where the payload begins.
/// Writing one that overstates makes every reader — this packet's own decoder, and the SRTP
/// transform that computes the header it must not encrypt — take media as header. Writing one that
/// understates makes them take header as media. Neither is a value `decode` can produce, so a
/// packet that carries one was built by hand on this side.
///
/// Refusing here rather than at a session boundary is what makes the answer reach every caller of
/// the packet layer and both legs, encrypted or not; `docs/specs/media-runtime.md` §4 argues the
/// choice. The extension is dropped and the payload is kept, which is what the four-octet filter
/// this extends has always done.
#[test]
fn encode_refuses_an_extension_that_disagrees_with_its_own_length_word() {
    // Eight octets carried under a length word claiming nine words — forty octets. Its header
    // would end inside the payload rather than past the packet, so nothing else rejects it.
    for declared in [0x00_09u16, 0x00_00, 0x00_02] {
        let mut extension = vec![0xBE, 0xDE];
        extension.extend_from_slice(&declared.to_be_bytes());
        extension.extend_from_slice(&[0x10, 0xAA, 0x00, 0x00]);

        let mut packet = Packet::new(0, 0x2A, 0x64, 0x07, Bytes::from_static(&[0xD5; 160]));
        packet.extension = Some(Bytes::from(extension));
        let encoded = packet.encode();

        let first = encoded.first().copied().expect("a header byte");
        assert_eq!(
            first & 0b0001_0000,
            0,
            "declaring {declared} words over eight octets: the extension bit is set over bytes \
             that do not add up, so the next reader takes media as header"
        );
        let read_back = Packet::decode(&encoded).expect("what was written decodes");
        assert_eq!(
            read_back.payload.as_ref(),
            [0xD5; 160].as_slice(),
            "declaring {declared} words over eight octets: the payload boundary moved"
        );
        assert_eq!(read_back.extension, None);
    }
}

/// The four-octet filter this generalises still holds: too short to hold a length word is the
/// total case of the same disagreement, and it was already dropped before `M-85`.
#[test]
fn encode_still_refuses_an_extension_with_no_length_word_in_it() {
    for short in [vec![], vec![0xBE], vec![0xBE, 0xDE], vec![0xBE, 0xDE, 0x00]] {
        let carried = short.len();
        let mut packet = Packet::new(0, 0x2A, 0x64, 0x07, Bytes::from_static(&[0xD5; 160]));
        packet.extension = Some(Bytes::from(short));
        let read_back = Packet::decode(&packet.encode()).expect("what was written decodes");

        assert_eq!(
            read_back.extension, None,
            "{carried} octets were written as an extension, and there is no length word in them"
        );
        assert_eq!(read_back.payload.as_ref(), [0xD5; 160].as_slice());
    }
}

/// And the well-formed one is still written, so the refusal is about the disagreement and not
/// about extensions.
#[test]
fn encode_still_writes_an_extension_that_agrees_with_itself() {
    for words in 0..=3u16 {
        let mut extension = vec![0xBE, 0xDE];
        extension.extend_from_slice(&words.to_be_bytes());
        extension.extend(std::iter::repeat_n(0xAAu8, usize::from(words) * 4));

        let mut packet = Packet::new(0, 0x2A, 0x64, 0x07, Bytes::from_static(&[0xD5; 160]));
        packet.extension = Some(Bytes::from(extension.clone()));
        let read_back = Packet::decode(&packet.encode()).expect("what was written decodes");

        assert_eq!(
            read_back.extension.as_deref(),
            Some(extension.as_slice()),
            "{words} well-formed word(s) were dropped"
        );
        assert_eq!(read_back.payload.as_ref(), [0xD5; 160].as_slice());
    }
}
