//! The RFC 4475 torture corpus, run against the parser.
//!
//! Each case says which layer must object to it and how. The parser is responsible for two of
//! the four classes:
//!
//! - `ParseOk` — must parse, and re-serialize to exactly the bytes that were framed.
//! - `ParseErr` — must be rejected, with the specific fault the case names.
//!
//! The other two classes (`HeaderErr`, `ValidateErr`) must *parse* here; the layers that
//! reject them assert on them in their own tests. That split is the point: a message with a
//! malformed `CSeq` frames and forwards perfectly well, and a parser that refused it would be
//! dropping messages a proxy is obliged to pass on.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use bytes::Bytes;
use sipx_sip::error::ParseError;
use sipx_sip::{Limits, Message, StreamParser, parse_datagram};
use sipx_testkit::rfc4475::{self, Expect, Fault};

fn parse(case: &rfc4475::Case) -> Result<Message, ParseError> {
    parse_datagram(Bytes::from_static(case.bytes), &Limits::datagram())
}

fn fault_of(err: &ParseError) -> Option<Fault> {
    match err {
        ParseError::StartLine(_) => Some(Fault::StartLine),
        ParseError::HeaderSyntax { .. } => Some(Fault::HeaderSyntax),
        ParseError::Framing(_) => Some(Fault::Framing),
        ParseError::Limit { .. } | ParseError::Incomplete => None,
    }
}

#[test]
fn valid_messages_parse() {
    for case in rfc4475::classified() {
        if !matches!(case.expect, Expect::ParseOk) {
            continue;
        }
        let parsed = parse(case);
        assert!(
            parsed.is_ok(),
            "RFC 4475 {} ({}) must parse, got {:?}\n---\n{}\n---",
            case.section,
            case.name,
            parsed.err(),
            case.lossy()
        );
    }
}

#[test]
fn valid_messages_reserialize_byte_exactly() {
    for case in rfc4475::classified() {
        if !matches!(case.expect, Expect::ParseOk) {
            continue;
        }
        let msg = parse(case).expect("already asserted to parse");
        let out = msg.to_bytes();

        assert!(
            case.bytes.starts_with(&out),
            "RFC 4475 {} ({}) did not re-serialize to a prefix of its input",
            case.section,
            case.name
        );

        if case.name == "dblreq" {
            // 3.1.1.8 is the one case where equality is the wrong assertion: the datagram
            // carries a REGISTER followed by octets that look like an INVITE, and the RFC
            // requires those to be ignored and *not* forwarded. Re-serializing to fewer bytes
            // than arrived is the correct behaviour here.
            assert!(
                out.len() < case.bytes.len(),
                "dblreq must drop its trailing octets"
            );
            continue;
        }

        assert_eq!(
            out.as_ref(),
            case.bytes,
            "RFC 4475 {} ({}) must round-trip byte for byte",
            case.section,
            case.name
        );
    }
}

#[test]
fn structurally_invalid_messages_are_rejected_with_the_right_fault() {
    for case in rfc4475::classified() {
        let Expect::ParseErr(expected) = case.expect else {
            continue;
        };
        let err = parse(case).err().unwrap_or_else(|| {
            panic!(
                "RFC 4475 {} ({}) must be rejected\n---\n{}\n---",
                case.section,
                case.name,
                case.lossy()
            )
        });
        assert_eq!(
            fault_of(&err),
            Some(expected),
            "RFC 4475 {} ({}) should fail as {:?}, got {err:?}",
            case.section,
            case.name,
            expected
        );
    }
}

/// Messages whose fault is in a header value, or in the set of headers, must still frame.
/// A proxy has to be able to forward what it cannot itself interpret.
#[test]
fn value_level_and_semantic_faults_still_frame() {
    for case in rfc4475::classified() {
        if !matches!(case.expect, Expect::HeaderErr(_) | Expect::ValidateErr(_)) {
            continue;
        }
        let parsed = parse(case);
        assert!(
            parsed.is_ok(),
            "RFC 4475 {} ({}) must frame — its fault belongs to a higher layer — got {:?}",
            case.section,
            case.name,
            parsed.err()
        );
    }
}

/// However a message is chopped up, it parses the same. Corpus messages are real ones, with
/// folding, odd whitespace and bodies, which makes them a far better stress test of the
/// incremental framer than anything hand-written.
#[test]
fn stream_framing_is_independent_of_chunk_boundaries() {
    for case in rfc4475::classified() {
        if !matches!(case.expect, Expect::ParseOk) {
            continue;
        }
        // Only messages with an explicit Content-Length can be framed on a stream at all.
        let mut whole = StreamParser::new(Limits::stream());
        let Ok(reference) = whole.push(case.bytes) else {
            continue;
        };
        if reference.len() != 1 {
            continue;
        }
        let reference = reference.first().map(Message::to_bytes);

        for split in 0..=case.bytes.len() {
            let mut parser = StreamParser::new(Limits::stream());
            let (a, b) = case.bytes.split_at(split);
            let Ok(mut got) = parser.push(a) else {
                panic!("{} failed on the first half at split {split}", case.name)
            };
            match parser.push(b) {
                Ok(rest) => got.extend(rest),
                Err(e) => panic!(
                    "{} failed on the second half at split {split}: {e:?}",
                    case.name
                ),
            }
            assert_eq!(
                got.first().map(Message::to_bytes),
                reference,
                "{} framed differently when split at {split}",
                case.name
            );
        }
    }
}

/// Nothing in the corpus, at any chunk boundary, may panic.
#[test]
fn no_corpus_message_panics_the_parser() {
    for case in rfc4475::CASES {
        let _ = parse_datagram(Bytes::from_static(case.bytes), &Limits::datagram());
        for split in 0..=case.bytes.len() {
            let mut parser = StreamParser::new(Limits::stream());
            let (a, b) = case.bytes.split_at(split);
            let _ = parser.push(a).and_then(|_| parser.push(b));
        }
    }
}
