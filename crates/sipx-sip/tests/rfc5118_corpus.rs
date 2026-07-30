//! The RFC 5118 IPv6 torture corpus, run against the parser.
//!
//! The shape mirrors `rfc4475_corpus.rs`, and deliberately so: the two corpora share the
//! `Expect` vocabulary, so a reader can put the files side by side and compare like with like.
//!
//! What differs is the balance. RFC 4475 is mostly rejections; RFC 5118 has exactly one, and its
//! §4.2 title says so. The other eleven messages are the RFC's demonstrations that a parser must
//! *accept* IPv6 constructs it may not expect — a bare `IPv6address` in a `received` parameter, a
//! port swallowed into a compressed reference, an extra colon the RFC 3261 ABNF permits by
//! accident. So the load-bearing assertion here is the converse one: nothing valid is refused.
//! A corpus that only proves rejections is half a test.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::IpAddr;

use sipx_sip::error::ParseError;
use sipx_sip::headers::Via;
use sipx_sip::uri::Host;
use sipx_sip::{Limits, Message, StreamParser, parse_datagram};
use sipx_testkit::rfc5118::{self, Case, Expect, Fault};

fn parse(case: &Case) -> Result<Message, ParseError> {
    parse_datagram(case.wire(), &Limits::datagram())
}

fn fault_of(err: &ParseError) -> Option<Fault> {
    match err {
        ParseError::StartLine(_) => Some(Fault::StartLine),
        ParseError::HeaderSyntax { .. } => Some(Fault::HeaderSyntax),
        ParseError::Framing(_) => Some(Fault::Framing),
        ParseError::Limit { .. } | ParseError::Incomplete => None,
    }
}

fn ip(literal: &str) -> IpAddr {
    literal.parse().expect("test literal is a valid address")
}

/// The address a request's R-URI names, or a description of why it does not name one.
fn request_uri_host(case: &Case) -> Host {
    let msg = parse(case).unwrap_or_else(|e| panic!("RFC 5118 {} must parse: {e:?}", case.name));
    let request = msg
        .as_request()
        .unwrap_or_else(|| panic!("RFC 5118 {} is a request", case.name));
    request
        .uri
        .host()
        .cloned()
        .unwrap_or_else(|| panic!("RFC 5118 {} R-URI must name a host", case.name))
}

/// The story's named assertion, and the one that decides whether this corpus is being run at
/// all: every message is classified, and every message behaves the way RFC 5118 says it does.
///
/// It is an umbrella on purpose. The per-section tests below pin down *what* sipx decided in the
/// cases where the RFC leaves a choice; this one pins down that each message was looked at and
/// landed on the side of accept/reject the RFC puts it on.
#[test]
fn every_rfc5118_message_is_classified_and_behaves_as_the_rfc_says() {
    assert_eq!(
        rfc5118::CASES.len(),
        12,
        "RFC 5118 Appendix A holds twelve messages across ten sections"
    );

    for case in rfc5118::CASES {
        assert!(
            case.is_classified(),
            "RFC 5118 {} ({}) is unclassified; every file in this archive is referenced by a section",
            case.section,
            case.name
        );

        match case.expect {
            Expect::ParseOk => {
                let parsed = parse(case);
                assert!(
                    parsed.is_ok(),
                    "RFC 5118 {} ({}) — {} — must parse, got {:?}\n---\n{}\n---",
                    case.section,
                    case.name,
                    case.title,
                    parsed.err(),
                    case.lossy()
                );
                // Accepting a message and then condemning it is the same failure wearing a
                // different hat, so the valid cases have to clear validation too.
                let msg = parsed.expect("just asserted");
                let findings: Vec<_> = sipx_sip::validate(&msg)
                    .into_iter()
                    .filter(|f| !f.is_repairable())
                    .collect();
                assert!(
                    findings.is_empty(),
                    "RFC 5118 {} ({}) is a valid message but validation objected: {findings:?}",
                    case.section,
                    case.name
                );
            }
            Expect::ParseErr(expected) => {
                let err = parse(case).err().unwrap_or_else(|| {
                    panic!(
                        "RFC 5118 {} ({}) — {} — must be rejected; the RFC says answer 400 Bad Request\n---\n{}\n---",
                        case.section,
                        case.name,
                        case.title,
                        case.lossy()
                    )
                });
                assert_eq!(
                    fault_of(&err),
                    Some(expected),
                    "RFC 5118 {} ({}) should fail as {:?}, got {err:?}",
                    case.section,
                    case.name,
                    expected
                );
            }
            Expect::HeaderErr(_) | Expect::ValidateErr(_) | Expect::Unreferenced => panic!(
                "RFC 5118 {} ({}) is classified {:?}; this corpus uses only ParseOk and ParseErr, \
                 and a new class needs its assertion written here before it is used",
                case.section, case.name, case.expect
            ),
        }
    }
}

/// The converse assertion `X-2` makes, made here too and stated separately because for this
/// corpus it is the whole point: eleven of the twelve messages are valid, and an IPv6 parser
/// fails by being too strict far more often than by being too lax.
#[test]
fn no_valid_message_in_the_corpus_is_rejected() {
    let refused: Vec<_> = rfc5118::expecting(Expect::ParseOk)
        .filter_map(|c| parse(c).err().map(|e| (c.section, c.name, e)))
        .collect();
    assert!(
        refused.is_empty(),
        "RFC 5118 messages the RFC calls valid were rejected: {refused:?}"
    );
    assert_eq!(
        rfc5118::expecting(Expect::ParseOk).count(),
        11,
        "eleven of the twelve are valid; if this drops, the assertion above got easier"
    );
}

/// §4.1 — the baseline. Correctly delimited references in the R-URI, the Via and the Contact.
#[test]
fn valid_ipv6_references_are_read_from_every_position() {
    let case = rfc5118::case("ipv6-good").expect("in corpus");
    assert_eq!(
        request_uri_host(case),
        Host::Ip(ip("2001:db8::10")),
        "§4.1 R-URI holds an IPv6 reference"
    );

    let msg = parse(case).expect("§4.1 parses");
    let via = msg
        .headers()
        .typed::<Via>()
        .expect("Via present")
        .expect("Via parses");
    assert_eq!(
        via.host,
        Host::Ip(ip("2001:db8::9:1")),
        "§4.1 Via sent-by holds an IPv6 reference"
    );
    assert_eq!(via.port, None, "§4.1 Via states no port");
}

/// §4.2 — the one invalid message: `sip:2001:db8::10`, an IPv6 address with the mandated `[`
/// and `]` stripped off. The RFC: "A SIP implementation receiving this request should respond
/// with a 400 Bad Request error."
///
/// Asserting the *fault* and not merely the failure is what stops this passing for the wrong
/// reason — a parser that rejected the message over its `Content-Length` would satisfy
/// `is_err()` while having missed the undelimited reference entirely.
#[test]
fn undelimited_ipv6_in_the_request_uri_is_refused() {
    let case = rfc5118::case("ipv6-bad").expect("in corpus");
    let err = parse(case).expect_err("§4.2 is titled invalid and must be refused");
    assert_eq!(
        fault_of(&err),
        Some(Fault::StartLine),
        "§4.2's fault is in the Request-URI, so it is a start-line fault: {err:?}"
    );

    // The contrast that makes the assertion mean something: the same message with the brackets
    // restored is §4.1, and it parses. So what is rejected is the missing delimiters and not
    // anything else about the message.
    let good = rfc5118::case("ipv6-good").expect("in corpus");
    assert!(
        parse(good).is_ok(),
        "§4.1 is §4.2 with the delimiters restored and must parse"
    );
}

/// §4.3 — the ambiguity, and the decision.
///
/// `sip:[2001:db8::10:5070]`. The sender meant "host 2001:db8::10, port 5070" and put the port
/// inside the `]`, where `::` expansion swallows it: the reference is a complete, legal IPv6
/// address whose last group is `5070`. The RFC is explicit that this is not a parse error —
/// "From a parsing perspective, the request below is well-formed. However, from a semantic point
/// of view, it will not yield the desired result."
///
/// **The decision sipx makes:** everything inside `[` `]` is the address, and a port is read only
/// after the `]`. So this URI names host `2001:db8::10:5070` with **no port**, and sipx will
/// contact it on the default port — not host `2001:db8::10` on port 5070.
///
/// This is the RFC's own reading, and it is the only one that keeps §4.3 and §4.4 distinct. It is
/// written down here because a message that parses two ways is not a bug to fix but a choice to
/// record, and an unrecorded choice is what makes two releases disagree.
#[test]
fn port_ambiguous_uri_takes_the_port_into_the_address() {
    let case = rfc5118::case("port-ambiguous").expect("in corpus");
    let msg = parse(case).expect("§4.3 is well-formed from a parsing perspective");
    let uri = &msg.as_request().expect("a REGISTER").uri;

    assert_eq!(
        uri.host().cloned(),
        Some(Host::Ip(ip("2001:db8::10:5070"))),
        "§4.3: the whole bracketed reference is the address — 5070 is its last group"
    );
    assert_eq!(
        uri.port(),
        None,
        "§4.3: no port is stated outside the ']', so the URI carries none"
    );

    // Reversal guard. The decision above is only meaningful if the *other* reading is a
    // different answer, so state what sipx must NOT have decided.
    assert_ne!(
        uri.host().cloned(),
        Some(Host::Ip(ip("2001:db8::10"))),
        "§4.3: the port must not have been split out of the reference"
    );
    assert_ne!(uri.port(), Some(5070), "§4.3: 5070 is address, not port");
}

/// §4.4 — the contrast that gives §4.3 its meaning: with the port outside the `]`, both halves
/// are read. Run together, the pair proves the `]` is what sipx keys on.
#[test]
fn port_unambiguous_uri_splits_host_from_port() {
    let case = rfc5118::case("port-unambiguous").expect("in corpus");
    let msg = parse(case).expect("§4.4 is well formatted");
    let uri = &msg.as_request().expect("a REGISTER").uri;

    assert_eq!(
        uri.host().cloned(),
        Some(Host::Ip(ip("2001:db8::10"))),
        "§4.4: the address ends at the ']'"
    );
    assert_eq!(uri.port(), Some(5070), "§4.4: the port follows the ']'");

    // The two sections differ by two characters and must not resolve alike.
    let ambiguous = rfc5118::case("port-ambiguous").expect("in corpus");
    let ambiguous_uri_host = request_uri_host(ambiguous);
    assert_ne!(
        ambiguous_uri_host,
        Host::Ip(ip("2001:db8::10")),
        "§4.3 and §4.4 must not parse to the same host, or the ']' is being ignored"
    );
}

/// §4.5 — a `received` parameter with and without the delimiters. RFC 3261's `via-received`
/// production takes a bare `IPv6address`, while `sent-by` takes a bracketed `IPv6reference`;
/// implementations split about 50/50 on what they sent. RFC 5118's instruction is the Robustness
/// Principle: "be liberal in accepting a 'received' parameter with or without the delimiting '['
/// and ']' tokens", and "A SIP implementation receiving either of these messages must parse them
/// successfully."
///
/// The bracketed form is the interesting half: it is *invalid* under a strict reading of the
/// grammar and must be accepted anyway. That is the one place in either corpus where "must
/// accept" and "matches the ABNF" come apart.
#[test]
fn via_received_is_accepted_with_and_without_delimiters() {
    for (name, received) in [
        ("via-received-param-no-delim", &b"2001:db8::9:255"[..]),
        ("via-received-param-with-delim", &b"[2001:db8::9:255]"[..]),
    ] {
        let case = rfc5118::case(name).expect("in corpus");
        let msg = parse(case).unwrap_or_else(|e| panic!("§4.5 {name} must parse: {e:?}"));
        let via = msg
            .headers()
            .typed::<Via>()
            .expect("Via present")
            .unwrap_or_else(|e| panic!("§4.5 {name}: the topmost Via must parse: {e:?}"));

        assert_eq!(
            via.host,
            Host::Ip(ip("2001:db8::9:1")),
            "§4.5 {name}: sent-by is a bracketed reference in both messages"
        );
        assert_eq!(
            via.received(),
            Some(received),
            "§4.5 {name}: the received parameter is preserved exactly as it arrived"
        );
    }
}

/// §4.7 — three Via hops mixing IPv4 and IPv6, one with a port outside the `]`, one with an
/// IPv4 `received`. Every hop must be readable: a proxy that can only parse the topmost Via
/// cannot route a response back through the list.
#[test]
fn mixed_ipv4_and_ipv6_via_list_parses_every_hop() {
    let case = rfc5118::case("mult-ip-in-header").expect("in corpus");
    let msg = parse(case).expect("§4.7 is valid and well-formed");
    let hops: Vec<Via> = msg
        .headers()
        .typed_all::<Via>()
        .map(|r| r.expect("every Via hop must parse"))
        .collect();

    assert_eq!(hops.len(), 3, "§4.7 carries three Via header fields");
    assert_eq!(hops[0].host, Host::Ip(ip("2001:db8::9:1")));
    assert_eq!(
        hops[0].port,
        Some(6050),
        "§4.7 hop 1 states a port outside the ']'"
    );
    assert_eq!(
        hops[1].host,
        Host::Ip(ip("192.0.2.1")),
        "§4.7 hop 2 is IPv4"
    );
    assert_eq!(hops[2].host, Host::Ip(ip("2001:db8::9:255")));
    assert_eq!(
        hops[2].received(),
        Some(&b"192.0.2.200"[..]),
        "§4.7 hop 3 carries an IPv4 received parameter"
    );
}

/// §4.9 — IPv4-mapped addresses in the signalling: two Vias and a Contact. "A SIP implementation
/// receiving a message that contains such a mapped address must be prepared to parse it
/// successfully." The topmost Via's port is outside the `]`, per §4.4.
#[test]
fn ipv4_mapped_addresses_parse_in_signalling() {
    let case = rfc5118::case("ipv4-mapped-ipv6").expect("in corpus");
    let msg = parse(case).expect("§4.9 is well-formed");
    let hops: Vec<Via> = msg
        .headers()
        .typed_all::<Via>()
        .map(|r| r.expect("every Via hop must parse"))
        .collect();

    assert_eq!(hops.len(), 2, "§4.9 carries two Via header fields");
    assert_eq!(
        hops[0].host,
        Host::Ip(ip("::ffff:192.0.2.10")),
        "§4.9 hop 1 is an IPv4-mapped address"
    );
    assert_eq!(hops[0].port, Some(19823), "§4.9 hop 1 states a port");
    assert_eq!(hops[1].host, Host::Ip(ip("::ffff:192.0.2.2")));
    assert_eq!(hops[1].port, None);
}

/// §4.10 — the ABNF bug. RFC 3261's grammar, inherited from the obsoleted RFC 2373, permits
/// `[2001:db8:::192.0.2.1]` with three colons before the embedded IPv4 address. RFC 4291 fixed
/// the grammar, but RFC 5118 requires tolerance of both: "following the Robustness Principle
/// [RFC1122], an implementation must tolerate both of the above constructs."
///
/// Both messages must parse, and both must name the *same* address — the extra colon is noise,
/// not a different host. That second assertion is the one with teeth: a parser that accepted the
/// three-colon form but read a different address out of it would satisfy "tolerate" while
/// silently routing somewhere else.
#[test]
fn the_rfc_3261_abnf_bug_reference_is_tolerated() {
    let expected = Host::Ip(ip("2001:db8::192.0.2.1"));

    let correct = rfc5118::case("ipv6-correct-abnf-2-colons").expect("in corpus");
    assert_eq!(
        request_uri_host(correct),
        expected,
        "§4.10: the two-colon form is the correct construct"
    );

    let buggy = rfc5118::case("ipv6-bug-abnf-3-colons").expect("in corpus");
    assert_eq!(
        request_uri_host(buggy),
        expected,
        "§4.10: the three-colon form must be tolerated and resolve to the same address"
    );
}

/// A valid message must re-serialize to exactly the bytes that were framed. For this corpus that
/// is a stronger claim than it looks: it says sipx does not silently rewrite an IPv6 reference on
/// the way through, including §4.10's extra colon. RFC 5118 permits a proxy to normalise that
/// colon away, but a parser that does it unasked has changed a message it was only forwarding.
#[test]
fn valid_messages_reserialize_byte_exactly() {
    for case in rfc5118::expecting(Expect::ParseOk) {
        let wire = case.wire();
        let msg = parse(case).unwrap_or_else(|e| panic!("{} must parse: {e:?}", case.name));
        assert_eq!(
            msg.to_bytes().as_ref(),
            wire.as_ref(),
            "RFC 5118 {} ({}) must round-trip byte for byte",
            case.section,
            case.name
        );
    }
}

/// However a message is chopped up, it parses the same.
#[test]
fn stream_framing_is_independent_of_chunk_boundaries() {
    for case in rfc5118::expecting(Expect::ParseOk) {
        let wire = case.wire();
        let mut whole = StreamParser::new(Limits::stream());
        let Ok(reference) = whole.push(&wire) else {
            continue;
        };
        if reference.len() != 1 {
            continue;
        }
        let reference = reference.first().map(Message::to_bytes);

        for split in 0..=wire.len() {
            let mut parser = StreamParser::new(Limits::stream());
            let (a, b) = wire.split_at(split);
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

/// Nothing in the corpus, at any chunk boundary, may panic. RFC 5118's messages are built to
/// break parsers that guess at where an IPv6 reference ends, so this is the assertion the corpus
/// exists to make — and it covers the bit-exact archive bytes as well as the wire form, because
/// LF-terminated input is exactly the sort of thing that arrives from a careless peer.
#[test]
fn no_corpus_message_panics_the_parser() {
    for case in rfc5118::CASES {
        for bytes in [case.wire(), bytes::Bytes::from_static(case.bytes)] {
            let _ = parse_datagram(bytes.clone(), &Limits::datagram());
            for split in 0..=bytes.len() {
                let mut parser = StreamParser::new(Limits::stream());
                let (a, b) = bytes.split_at(split);
                let _ = parser.push(a).and_then(|_| parser.push(b));
            }
        }
    }
}
