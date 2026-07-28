//! Negotiating SRTP, and refusing to negotiate it badly.
//!
//! Three outcomes matter and only one of them is the happy path: a secure offer answered with a
//! key, a secure offer this side cannot key **declined**, and a plain offer answered plainly
//! even when this side would have preferred encryption.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fmt::Write as _;
use std::net::IpAddr;

use sipx_sdp::crypto::{Crypto, Suite};
use sipx_sdp::{Capabilities, answer, parse};

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("valid")
}

fn offer(protocol: &str, attributes: &[String]) -> String {
    let mut sdp = format!(
        "v=0\r\no=- 1 1 IN IP4 192.0.2.1\r\ns=-\r\nc=IN IP4 192.0.2.1\r\nt=0 0\r\n\
         m=audio 40000 {protocol} 0\r\na=rtpmap:0 PCMU/8000\r\n"
    );
    for attribute in attributes {
        let _ = writeln!(sdp, "a={attribute}\r");
    }
    sdp
}

fn a_key() -> Crypto {
    Crypto::offer(1, Suite::AesCm128HmacSha1_80, true).expect("secure")
}

#[test]
fn a_secure_offer_is_answered_with_a_key() {
    let their_key = a_key();
    let offered = parse(&offer(
        "RTP/SAVP",
        &[format!("crypto:{}", their_key.to_value())],
    ))
    .expect("parses");

    let answered = answer(
        &offered,
        &Capabilities::g711(loopback(), 40_002).with_srtp(true),
    );
    let stream = &answered.media[0];

    assert_ne!(stream.port, 0, "the stream must not be declined");
    let ours = stream.crypto().expect("the answer carries a key");
    assert_eq!(ours.suite, Suite::AesCm128HmacSha1_80);
    assert_ne!(
        ours.key_and_salt, their_key.key_and_salt,
        "each direction has its own key; sharing one gives both ends the same keystream"
    );
}

/// A secure offer this side cannot key is **declined**, not answered in the clear. Answering
/// `RTP/SAVP` without a key promises encryption neither side can perform; answering `RTP/AVP` is
/// a downgrade this side chose on the caller's behalf.
#[test]
fn a_secure_offer_that_cannot_be_keyed_is_declined() {
    let offered = parse(&offer(
        "RTP/SAVP",
        &[format!("crypto:{}", a_key().to_value())],
    ))
    .expect("parses");

    // Cleartext signalling, so `with_srtp` produces no key at all.
    let answered = answer(
        &offered,
        &Capabilities::g711(loopback(), 40_002).with_srtp(false),
    );
    assert_eq!(answered.media[0].port, 0, "declined rather than downgraded");
}

/// And a secure offer that carries no key at all — `RTP/SAVP` with nothing to key it with.
#[test]
fn a_secure_offer_with_no_key_is_declined() {
    let offered = parse(&offer("RTP/SAVP", &[])).expect("parses");
    let answered = answer(
        &offered,
        &Capabilities::g711(loopback(), 40_002).with_srtp(true),
    );
    assert_eq!(answered.media[0].port, 0);
}

/// A plain offer stays plain. Answering `RTP/AVP` with `a=crypto` is how a stream ends up
/// encrypted at one end and not the other.
#[test]
fn a_plain_offer_is_not_answered_with_a_key() {
    let offered = parse(&offer("RTP/AVP", &[])).expect("parses");
    let answered = answer(
        &offered,
        &Capabilities::g711(loopback(), 40_002).with_srtp(true),
    );
    let stream = &answered.media[0];

    assert_ne!(stream.port, 0, "a plain call is still a call");
    assert!(stream.crypto().is_none(), "no key in a plain answer");
    assert!(!stream.protocol.contains("SAVP"), "{}", stream.protocol);
}

/// The offer this side makes says `RTP/SAVP` only when it has a key to back it.
#[test]
fn the_offered_protocol_follows_the_key() {
    assert_eq!(
        Capabilities::g711(loopback(), 1).with_srtp(true).protocol(),
        "RTP/SAVP"
    );
    assert_eq!(
        Capabilities::g711(loopback(), 1)
            .with_srtp(false)
            .protocol(),
        "RTP/AVP"
    );
    assert_eq!(Capabilities::g711(loopback(), 1).protocol(), "RTP/AVP");
}

/// Several suites may be offered in preference order; sipx takes the first it can perform rather
/// than the first listed.
#[test]
fn an_offer_is_answered_even_when_its_favourite_suite_is_unsupported() {
    let their_key = a_key();
    let offered = parse(&offer(
        "RTP/SAVP",
        &[
            "crypto:1 AES_256_CM_HMAC_SHA1_80 inline:AAAAAAAA".to_owned(),
            format!("crypto:{}", their_key.to_value()),
        ],
    ))
    .expect("parses");

    let answered = answer(
        &offered,
        &Capabilities::g711(loopback(), 40_002).with_srtp(true),
    );
    assert_ne!(
        answered.media[0].port, 0,
        "an unsupported first choice is not a refusal"
    );
    assert!(answered.media[0].crypto().is_some());
}
