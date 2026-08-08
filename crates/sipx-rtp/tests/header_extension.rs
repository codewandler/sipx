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
