//! A diagnostic record of a packet says what it is, never what it carries (`M-110`).
//!
//! This is the far end of the relay path `M-107` fixed at `sipx_media::Encoded`, and the two
//! halves are one defect: a derived `Debug` renders the whole buffer, so a `tracing` field, an
//! `expect` message or a test failure carrying a packet puts the call — or a participant's name —
//! into a record whose length is the packet's. `M-80` marked `Encoded` and [`Packet`]
//! `#[non_exhaustive]` together "because they are the two ends of one relay path"; the same
//! sentence applies to their renderings.
//!
//! **Two sensitivities, treated on their own terms rather than under one rule.**
//!
//! `Packet::payload` is the conversation still encoded, and for G.711 an octet *is* a sample.
//! [`SdesItem::value`] is not audio at all: RFC 3550 §6.5 makes it a `CNAME`, a `NAME` or an
//! `EMAIL` — a login name, somebody's real name, somebody's email address — which is personal data
//! about a participant rather than the call between them. Both end up redacted here, for reasons
//! that do not share a premise, and each type's own documentation carries its half of the argument.
//!
//! What every assertion below has in common is the second half of the defect: a record whose
//! length is the value's is an unbounded diagnostic, so each rendering is bounded by the
//! implementation and one test builds a hostile value to prove it.

#![allow(clippy::expect_used, clippy::indexing_slicing)]

use bytes::{BufMut, Bytes, BytesMut};
use sipx_rtp::rtcp::{GOODBYE, Rtcp, SDES_CNAME, Sdes, SdesChunk, SdesItem};
use sipx_rtp::{Packet, ReceiverReport, ReportBlock};

/// The longest record any of these types may write, and the bound `M-107` used at `Encoded`.
///
/// A constant rather than a multiple of anything: the whole property is that the length of a
/// record does not follow the length of the thing it describes.
const BOUND: usize = 200;

/// Assert that an octet appears in a record in none of the spellings a `Debug` can write it in.
///
/// `Bytes` renders as `b"\xa7…"` and a `Vec<u8>` as `[167, …]`, so a check for one spelling passes
/// against the other. Both hex cases, because `{:x}` and `{:X}` are each one formatter apart — and
/// the printable run, because `Bytes` writes a graphic octet as **itself** and neither numeric
/// spelling appears at all. A `0x7E` payload rendered `b"~~~~…"` past the first version of this
/// helper, which is the false pass a redaction check cannot afford.
fn carries_no_octet(record: &str, octet: u8, what: &str) {
    let mut spellings = vec![
        format!("{octet}"),
        format!("{octet:02x}"),
        format!("{octet:02X}"),
    ];
    if octet.is_ascii_graphic() {
        // Four in a row: one graphic character is a field name away from a false failure, and a
        // payload is never one octet long in any fixture here.
        spellings.push(char::from(octet).to_string().repeat(4));
    }
    for spelling in spellings {
        assert!(
            !record.contains(&spelling),
            "{what} survived as `{spelling}` in: {record}"
        );
    }
}

/// A packet whose payload is one distinctive octet repeated, with an extension beside it.
///
/// `0xA7` is 167 in decimal and `a7` in hex, and neither spelling is a payload type, a sequence
/// number, a timestamp, an SSRC or a length anywhere in this fixture — so finding either in a
/// record can only mean the payload itself was rendered.
fn packet_with_a_distinctive_payload() -> Packet {
    let mut packet = Packet::new(
        96,
        1000,
        987_654,
        0xDEAD_BEEF,
        Bytes::from(vec![0xA7u8; 160]),
    );
    // RFC 8285's one-byte profile, one word of data. `0xC3` is 195 and `c3`, distinct from the
    // payload's octet so a record cannot confuse the two.
    let mut extension = BytesMut::new();
    extension.put_u16(0xBEDE);
    extension.put_u16(1);
    extension.put_slice(&[0xC3; 4]);
    packet.extension = Some(extension.freeze());
    packet.csrc = vec![0x1111_2222, 0x3333_4444];
    packet.marker = true;
    packet
}

/// The failing-first test for `M-110`: a packet's `Debug` carries its header and not its media.
///
/// What a relay log is for is on the left of this — which codec, which stream, where in the
/// stream, and how much audio there was. What it is not for is the audio.
#[test]
fn a_packet_renders_its_header_and_not_its_payload() {
    let packet = packet_with_a_distinctive_payload();
    let record = format!("{packet:?}");

    carries_no_octet(&record, 0xA7, "a payload octet");
    carries_no_octet(&record, 0xC3, "a header extension octet");

    assert!(
        record.contains("96"),
        "the payload type is what a relay log is for: {record}"
    );
    assert!(
        record.contains("1000"),
        "and where in the stream it sat: {record}"
    );
    assert!(
        record.contains("987654"),
        "and its sampling instant: {record}"
    );
    assert!(
        record.contains(&0xDEAD_BEEFu32.to_string()),
        "and whose stream it is: {record}"
    );
    assert!(
        record.contains("true"),
        "and whether it marked an event: {record}"
    );
    assert!(
        record.contains("160"),
        "and how much audio there was: {record}"
    );
}

/// The second half of the same defect: a redaction that still grew with the packet fixes nothing.
///
/// Every part of a `Packet` a caller can make arbitrarily long is here at once — payload, header
/// extension and the contributing-source list, which the wire caps at fifteen and the field does
/// not. A rendering that names a count for each is bounded by this implementation; one that names
/// the values is bounded by whatever arrived.
#[test]
fn a_packet_record_is_bounded_independently_of_the_packet() {
    let mut packet = Packet::new(96, 1, 1, 1, Bytes::from(vec![0xA7u8; 100_000]));
    packet.extension = Some(Bytes::from(vec![0xC3u8; 40_004]));
    packet.csrc = vec![0x1111_2222; 10_000];

    let record = format!("{packet:?}");
    assert!(
        record.len() < BOUND,
        "a record whose length is the packet's is an unbounded diagnostic: {} octets",
        record.len()
    );
    carries_no_octet(&record, 0xA7, "a payload octet");
}

/// RFC 3550 §6.5's items are personal data, and the argument is not the payload's.
///
/// A `CNAME` is §6.5.1's `user@host` built from a login name, a `NAME` is §6.5.2's "John Doe" and
/// an `EMAIL` is §6.5.3's address — none of it is audio, and all of it identifies a human. The
/// item's *type* stays in the record, because "the peer sent a NAME item" is a protocol fact and
/// is what an operator actually needs to see.
#[test]
fn a_source_description_item_renders_its_type_and_not_its_value() {
    for (kind, value) in [
        (SDES_CNAME, &b"alice.smith@host.invalid"[..]),
        (2, &b"Alice Smith"[..]),
        (3, &b"alice.smith@example.invalid"[..]),
    ] {
        let item = SdesItem {
            kind,
            value: Bytes::copy_from_slice(value),
        };
        let record = format!("{item:?}");
        assert!(
            !record.contains("alice") && !record.contains("Alice"),
            "an RFC 3550 §6.5 item's value is personal data and stayed in the record: {record}"
        );
        assert!(
            record.contains(&format!("kind: {kind}")),
            "which item it is belongs in the record: {record}"
        );
        assert!(
            record.contains(&value.len().to_string()),
            "and how long it was: {record}"
        );
    }
}

/// The same, through the packet that carries the items — a redaction one type up is not one.
#[test]
fn a_source_description_packet_renders_no_item_value() {
    let sdes = Rtcp::Sdes(Sdes {
        chunks: vec![SdesChunk {
            ssrc: 0x1111_2222,
            items: vec![
                SdesItem {
                    kind: SDES_CNAME,
                    value: Bytes::from_static(b"alice.smith@host.invalid"),
                },
                SdesItem {
                    kind: 3,
                    value: Bytes::from_static(b"alice.smith@example.invalid"),
                },
            ],
        }],
    });
    let record = format!("{sdes:?}");
    assert!(
        !record.contains("alice"),
        "a participant's identity reached a record through the packet: {record}"
    );
    assert!(
        record.contains(&0x1111_2222u32.to_string()),
        "whose description it is stays: {record}"
    );
}

/// An RTCP packet this crate does not model is bytes it cannot classify, so it renders none of
/// them.
///
/// RFC 3550 §6.6's `BYE` carries an optional free-text reason a participant typed and §6.7's `APP`
/// carries whatever a profile put there. The variant exists so a compound can be forwarded intact,
/// which is a reason to keep the bytes and not a reason to print them.
#[test]
fn an_unmodelled_rtcp_packet_renders_its_type_and_not_its_body() {
    let other = Rtcp::Other {
        packet_type: GOODBYE,
        count: 1,
        padding: false,
        payload: Bytes::from(vec![0x7Eu8; 40]),
    };
    let record = format!("{other:?}");

    carries_no_octet(&record, 0x7E, "an unmodelled packet's body octet");
    assert!(
        record.contains("203"),
        "which packet type it was is what a forwarding log is for: {record}"
    );
    assert!(record.contains("40"), "and how long its body was: {record}");
}

/// What a report says is *not* redacted, and this pins that decision so it is not undone by
/// somebody generalising the redaction above it.
///
/// A report block is seven integer counters describing a stream — loss, jitter, the highest
/// sequence number seen. None of it is the call and none of it is a participant; it is the entire
/// reason RTCP exists, and a log that could not read it could not say why a call sounded bad.
#[test]
fn a_receiver_report_still_renders_its_numbers() {
    let mut receiver = ReceiverReport::new(9);
    receiver.reports.push(ReportBlock {
        ssrc: 0x1234_5678,
        fraction_lost: 26,
        cumulative_lost: 42,
        extended_highest_sequence: 0x0001_0064,
        jitter: 17,
        last_sender_report: 0xAABB_CCDD,
        delay_since_last_sender_report: 65_536,
    });
    let record = format!("{:?}", Rtcp::Receiver(receiver));
    assert!(record.contains("26"), "the loss fraction: {record}");
    assert!(record.contains("17"), "the jitter estimate: {record}");
}
