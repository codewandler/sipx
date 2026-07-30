//! The SDP half of the RFC 5118 IPv6 torture corpus.
//!
//! RFC 5118 §4.6, §4.8 and §4.9 carry SDP bodies, and IPv6 in a `c=` or `o=` line is a different
//! code path from IPv6 in a `Via`: SDP has its own grammar (RFC 8866 §5.7) and never adopted the
//! `[` `]` delimiters that RFC 3261 mandates for a SIP URI. A stack that reuses its SIP host
//! parser for a `c=` line rejects every one of these bodies, and a stack that reuses its SDP
//! parser for a `Via` accepts references it should not. Only running both halves catches either.
//!
//! The bodies are read from the bit-exact corpus at
//! `crates/sipx-testkit/corpus/rfc5118/`, imported from the RFC's Appendix A archive by
//! `scripts/import-rfc5118-corpus.sh`. They are read at run time rather than pulled in with
//! `include_bytes!` deliberately: `sipx-sdp` is a published crate, and a compile-time include
//! reaching outside its own directory would not survive being packaged. The classification for
//! the corpus, and the `sipx-sip` half of the harness, live in `sipx-testkit`.

// A test that cannot read its own fixtures should fail loudly — AGENTS.md non-negotiable 3.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::IpAddr;

use sipx_sdp::session::{Address, SessionDescription};

/// The SDP body of a corpus message.
///
/// The archive's files are LF-terminated (see the corpus README), and `sipx-sdp` is deliberately
/// lenient about line endings, so the body is handed over as it is stored. What matters here is
/// that the octets of every address come from the RFC and not from a transcription.
fn sdp_body(name: &str) -> String {
    let path = format!(
        "{}/../sipx-testkit/corpus/rfc5118/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read(&path).unwrap_or_else(|e| panic!("reading the corpus at {path}: {e}"));
    let text = String::from_utf8(raw).expect("RFC 5118's messages are ASCII");
    let (_headers, body) = text
        .split_once("\n\n")
        .unwrap_or_else(|| panic!("{name} should carry a body separated by a blank line"));
    body.to_owned()
}

fn parse(name: &str) -> SessionDescription {
    sipx_sdp::parse(&sdp_body(name))
        .unwrap_or_else(|e| panic!("RFC 5118 {name}: its SDP body must parse, got {e:?}"))
}

fn ip(literal: &str) -> IpAddr {
    literal.parse().expect("test literal is a valid address")
}

/// §4.6 — "SIP Request with IPv6 Addresses in Session Description Protocol (SDP) Body". The RFC
/// calls the request "valid and well-formed", and notes explicitly that the IPv6 addresses in the
/// SDP body do not have the delimiting `[` and `]`.
#[test]
fn ipv6_in_session_level_origin_and_connection() {
    let sdp = parse("ipv6-in-sdp");

    assert_eq!(
        sdp.origin.address,
        Address::Ip(ip("2001:db8::20")),
        "§4.6: the o= line names an undelimited IPv6 address"
    );
    assert_eq!(
        sdp.connection.as_ref().map(|c| c.address.clone()),
        Some(Address::Ip(ip("2001:db8::20"))),
        "§4.6: the session-level c= line names an undelimited IPv6 address"
    );

    // Both streams inherit the session-level address, which is what makes this a session-level
    // test rather than a per-media one.
    assert_eq!(sdp.media.len(), 2, "§4.6 offers audio and video");
    for media in &sdp.media {
        assert_eq!(
            sdp.address_for(media),
            Some(ip("2001:db8::20")),
            "§4.6: the {} stream inherits the session's IPv6 address",
            media.media
        );
    }
    assert_eq!(sdp.media[0].port, 6000, "§4.6 audio is on 6000");
    assert_eq!(sdp.media[1].port, 6024, "§4.6 video is on 6024");
}

/// §4.8 — "Multiple IP Addresses in SDP". "The SDP contains multiple media lines, and each media
/// line is identified by a different network connection address."
///
/// The mix is the test: there is no session-level `c=` at all, the `o=` line names a *hostname*
/// rather than a literal, and the two streams carry an IPv4 and an IPv6 `c=` respectively. A
/// parser that let a session-level default stand in, or that assumed one address family per
/// description, sends media to the wrong place for one of the two streams.
#[test]
fn per_media_connection_lines_mix_address_families() {
    let sdp = parse("mult-ip-in-sdp");

    assert_eq!(
        sdp.origin.address,
        Address::Host("host.example.com".to_owned()),
        "§4.8: the o= line names a hostname, not a literal"
    );
    assert_eq!(
        sdp.origin.address.ip(),
        None,
        "§4.8: a name is not an address, and only a resolver could make it one"
    );
    assert!(
        sdp.connection.is_none(),
        "§4.8: there is no session-level c= line; each stream carries its own"
    );

    assert_eq!(sdp.media.len(), 2, "§4.8 offers audio and video");
    assert_eq!(
        sdp.address_for(&sdp.media[0]),
        Some(ip("192.0.2.1")),
        "§4.8: the audio stream's own c= line is IPv4"
    );
    assert_eq!(
        sdp.address_for(&sdp.media[1]),
        Some(ip("2001:db8::1")),
        "§4.8: the video stream's own c= line is IPv6"
    );

    // The reversal guard: the two streams must not resolve to the same address, which is what
    // would happen if per-media c= lines were being ignored in favour of a single default.
    assert_ne!(
        sdp.address_for(&sdp.media[0]),
        sdp.address_for(&sdp.media[1]),
        "§4.8: each media line is identified by a *different* connection address"
    );
}

/// §4.9 — "IPv4-Mapped IPv6 Addresses". "An IPv4-mapped IPv6 address may appear in signaling, or
/// in the SDP carried by the signaling message, or in both." Here it appears in both; this test
/// covers the SDP, and `sipx-sip`'s `rfc5118_corpus.rs` covers the Vias and the Contact.
#[test]
fn ipv4_mapped_addresses_parse_in_sdp() {
    let sdp = parse("ipv4-mapped-ipv6");
    let mapped = ip("::ffff:192.0.2.2");

    assert_eq!(
        sdp.origin.address,
        Address::Ip(mapped),
        "§4.9: the o= line carries an IPv4-mapped address"
    );
    assert_eq!(
        sdp.connection.as_ref().map(|c| c.address.clone()),
        Some(Address::Ip(mapped)),
        "§4.9: so does the c= line"
    );

    // An IPv4-mapped address must stay an IPv6 address. Silently unwrapping it to 192.0.2.2
    // would change which socket family the media stack opens, so assert the family survived.
    assert!(
        matches!(sdp.origin.address.ip(), Some(IpAddr::V6(_))),
        "§4.9: ::ffff:192.0.2.2 is an IPv6 address and must not be collapsed to IPv4"
    );
    assert_eq!(sdp.media.len(), 2, "§4.9 offers audio and video");
}

/// Every SDP body in the corpus round-trips: what sipx writes back parses to the same
/// description. RFC 5118's addresses are the awkward part of that claim — a serializer that
/// bracketed an IPv6 address in a `c=` line, the way a SIP URI requires, would produce something
/// its own parser rejects.
#[test]
fn every_sdp_body_in_the_corpus_round_trips() {
    for name in ["ipv6-in-sdp", "mult-ip-in-sdp", "ipv4-mapped-ipv6"] {
        let sdp = parse(name);
        let written = sdp.to_string_sdp();

        assert!(
            !written.contains('[') && !written.contains(']'),
            "RFC 5118 {name}: SDP addresses take no '[' ']' delimiters, but sipx wrote:\n{written}"
        );

        let reparsed = sipx_sdp::parse(&written)
            .unwrap_or_else(|e| panic!("RFC 5118 {name}: sipx's own output must parse, got {e:?}"));
        assert_eq!(
            reparsed, sdp,
            "RFC 5118 {name}: the description must survive a write/read round trip"
        );
    }
}
