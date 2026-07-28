//! Property tests for message parsing.
//!
//! These assert the same invariants as the fuzz targets in `fuzz/`, so contributors without a
//! nightly toolchain still get coverage of them on every `cargo test`. The fuzzer explores far
//! deeper; this is the floor, not the ceiling.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use bytes::Bytes;
use proptest::prelude::*;
use sipx_sip::{Limits, Message, StreamParser, parse_datagram};
use sipx_testkit::rfc4475;

/// Bytes biased toward things that look like SIP, so the generator spends its time near the
/// grammar rather than in random noise that fails at the first byte.
fn sipish_bytes() -> impl Strategy<Value = Vec<u8>> {
    let fragments: Vec<&'static [u8]> = vec![
        b"INVITE sip:a@b.com SIP/2.0",
        b"SIP/2.0 200 OK",
        b"\r\n",
        b"Via: SIP/2.0/UDP h;branch=z9hG4bKx",
        b"Content-Length: 5",
        b"Content-Length: -1",
        b"Content-Length: 99999999999999999999",
        b"To: <sip:a@b>",
        b"  folded",
        b"\r\n\r\n",
        b"hello",
        b":",
        b" ",
        b"\r",
        b"\n",
        b"\x00",
    ];
    proptest::collection::vec(0..fragments.len(), 0..24).prop_map(move |picks| {
        let mut out = Vec::new();
        for i in picks {
            if let Some(f) = fragments.get(i) {
                out.extend_from_slice(f);
            }
        }
        out
    })
}

proptest! {
    /// Whatever arrives, the parser returns rather than panicking. It is reachable from the
    /// network by definition.
    #[test]
    fn datagram_parsing_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = parse_datagram(Bytes::from(bytes), &Limits::datagram());
    }

    #[test]
    fn sipish_datagram_parsing_never_panics(bytes in sipish_bytes()) {
        let _ = parse_datagram(Bytes::from(bytes), &Limits::datagram());
    }

    /// A message that parses must serialize to a prefix of what it was parsed from, and that
    /// output must parse again to the same bytes. Without this a proxy silently rewrites the
    /// messages it forwards.
    #[test]
    fn parsing_is_a_fixed_point(bytes in sipish_bytes()) {
        let input = Bytes::from(bytes);
        let limits = Limits::datagram();
        if let Ok(message) = parse_datagram(input.clone(), &limits) {
            let out = message.to_bytes();
            prop_assert!(
                input.starts_with(&out),
                "serialized to something that is not a prefix of the input"
            );
            let reparsed = parse_datagram(out.clone(), &limits)
                .expect("the output of a successful parse must itself parse");
            prop_assert_eq!(Message::to_bytes(&reparsed), out);
        }
    }

    /// Stream framing must not depend on how the bytes were chunked.
    #[test]
    fn stream_framing_never_panics(
        bytes in sipish_bytes(),
        chunk in 1usize..32,
    ) {
        let mut parser = StreamParser::new(Limits::stream());
        for piece in bytes.chunks(chunk) {
            if parser.push(piece).is_err() {
                break;
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Corpus messages with arbitrary bytes appended must never yield *more* than the message
    /// that was framed — appending noise to a datagram cannot conjure a second message.
    #[test]
    fn appending_noise_to_a_datagram_changes_nothing(
        index in 0usize..rfc4475::CASES.len(),
        noise in proptest::collection::vec(any::<u8>(), 0..64),
    ) {
        let Some(case) = rfc4475::CASES.get(index) else {
            return Ok(());
        };
        let limits = Limits::datagram();
        let clean = parse_datagram(Bytes::from_static(case.bytes), &limits)
            .map(|m| m.to_bytes());

        let mut extended = case.bytes.to_vec();
        extended.extend_from_slice(&noise);
        let dirty = parse_datagram(Bytes::from(extended), &limits).map(|m| m.to_bytes());

        // Only messages with an explicit Content-Length are unaffected: without one the body
        // runs to the end of the datagram by definition, so appending really does change it.
        let has_length = case
            .bytes
            .windows(15)
            .any(|w| w.eq_ignore_ascii_case(b"content-length:"));
        if has_length && clean.is_ok() {
            prop_assert_eq!(
                clean.ok(),
                dirty.ok(),
                "{} framed differently with noise appended",
                case.name
            );
        }
    }
}
