//! Verbatim passthrough: the guarantee a proxy is built on.
//!
//! sipx must forward what it does not understand exactly as it arrived. A stack that
//! normalizes header spelling or whitespace on the way through breaks signature-bearing
//! headers, defeats packet-capture comparison, and silently rewrites extensions it has never
//! heard of.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use bytes::Bytes;
use sipx_sip::name::HeaderName;
use sipx_sip::{Header, Limits, Message, parse_datagram};
use sipx_testkit::rfc4475::{self, Expect};

fn parse(text: &[u8]) -> Message {
    parse_datagram(Bytes::copy_from_slice(text), &Limits::datagram()).expect("should parse")
}

/// Headers sipx has no type for survive with their bytes, their spelling, their odd spacing
/// and their folding intact.
#[test]
fn unknown_headers_survive_roundtrip_byte_exact() {
    // Every oddity here is drawn from RFC 4475 §3.1.1.1, which is a legal message.
    let text: &[u8] = b"OPTIONS sip:user@example.com SIP/2.0\r\n\
        Via: SIP/2.0/UDP h.example.com;branch=z9hG4bKx\r\n\
        To: <sip:user@example.com>\r\n\
        From: <sip:caller@example.net>;tag=1\r\n\
        Call-ID: passthrough.1@example.com\r\n\
        CSeq: 1 OPTIONS\r\n\
        Max-Forwards: 70\r\n\
        NewFangledHeader:   newfangled value\r\n continued newfangled value\r\n\
        UnknownHeaderWithUnusualValue: ;;,,;;,;\r\n\
        Content-Length   : 0\r\n\r\n";

    let msg = parse(text);
    assert_eq!(
        msg.to_bytes().as_ref(),
        text,
        "an unmodified message must be re-emitted byte for byte"
    );

    // The unknown headers are present, with their names spelled as they arrived.
    let unknown: Vec<_> = msg
        .headers()
        .iter()
        .filter(|h| matches!(h.name(), HeaderName::Other(_)))
        .collect();
    assert_eq!(unknown.len(), 2);
    assert_eq!(unknown[0].name().canonical(), b"NewFangledHeader");
    // Folding is preserved in the raw value and collapsed only in the parsed view.
    assert_eq!(
        unknown[0].raw_value(),
        b"newfangled value\r\n continued newfangled value"
    );
    assert_eq!(
        unknown[0].value().as_ref(),
        b"newfangled value continued newfangled value"
    );
    // The junk value under an unknown name is perfectly legal, unlike the same value in a Via.
    assert_eq!(unknown[1].value().as_ref(), b";;,,;;,;");
}

/// Adding a header leaves every other header exactly as it was, so only the change is visible
/// in the output.
#[test]
fn adding_a_header_does_not_disturb_the_others() {
    let text: &[u8] = b"OPTIONS sip:user@example.com SIP/2.0\r\n\
        Via: SIP/2.0/UDP first.example.com;branch=z9hG4bK1\r\n\
        WeIrDlY-SpElLeD  :  and oddly spaced\r\n\
        Content-Length: 0\r\n\r\n";

    let mut msg = parse(text);
    msg.headers_mut().push_front(Header::new(
        HeaderName::Via,
        Bytes::from_static(b"SIP/2.0/UDP proxy.example.com;branch=z9hG4bK2"),
    ));

    let out = msg.to_bytes();
    let expected: &[u8] = b"OPTIONS sip:user@example.com SIP/2.0\r\n\
        Via: SIP/2.0/UDP proxy.example.com;branch=z9hG4bK2\r\n\
        Via: SIP/2.0/UDP first.example.com;branch=z9hG4bK1\r\n\
        WeIrDlY-SpElLeD  :  and oddly spaced\r\n\
        Content-Length: 0\r\n\r\n";
    assert_eq!(out.as_ref(), expected);
}

/// The same property across the whole corpus, header by header rather than message by
/// message: whatever a header's original bytes were, they come back.
#[test]
fn every_corpus_header_re_emits_verbatim() {
    for case in rfc4475::classified() {
        if !matches!(case.expect, Expect::ParseOk) {
            continue;
        }
        let msg = parse(case.bytes);
        for header in msg.headers().iter() {
            let mut out = Vec::new();
            header.write_to(&mut out);
            // The field line, as written back, must occur in the original message.
            assert!(
                case.bytes.windows(out.len()).any(|w| w == out.as_slice()),
                "{}: header {} was not re-emitted verbatim:\n  {:?}",
                case.name,
                header.name(),
                String::from_utf8_lossy(&out)
            );
        }
    }
}
