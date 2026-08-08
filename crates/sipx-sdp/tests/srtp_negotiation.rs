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

// -------------------------------------------------------------------------------------------
// The tag, which RFC 4568 requires twice: echoed in the answer (§5.1.2) and verified when the
// answer comes back (§5.1.3). `docs/specs/srtp.md` §5.3, §5.4 and §12.3.
// -------------------------------------------------------------------------------------------

/// RFC 4568 §5.1.2: the accepted attribute in the answer "MUST contain … the tag and
/// crypto-suite from the accepted crypto attribute in the offer".
///
/// The failure this prevents is one-sided and therefore easy to miss: a conformant peer that
/// offered any tag but 1 MUST fail the negotiation on an answer carrying 1, and nothing at this
/// end reports anything. Calls to peers that happen to use tag 1 — most of them — work.
#[test]
fn the_answer_echoes_the_tag_of_the_accepted_offer() {
    let their_key = Crypto::offer(9, Suite::AesCm128HmacSha1_80, true).expect("secure");
    let offered = parse(&offer(
        "RTP/SAVP",
        &[format!("crypto:{}", their_key.to_value())],
    ))
    .expect("parses");

    let answered = answer(
        &offered,
        &Capabilities::g711(loopback(), 40_002).with_srtp(true),
    );
    let ours = answered.media[0]
        .crypto()
        .expect("the answer carries a key");

    assert_eq!(
        ours.tag, 9,
        "the answer must echo the offer's tag, not this side's own"
    );
    assert_eq!(ours.suite, their_key.suite, "and the offer's suite");
    assert_ne!(
        ours.key_and_salt, their_key.key_and_salt,
        "the tag is echoed; the key is still this side's own"
    );
}

/// The tag echoed is the one on the attribute **actually accepted**, not the offer's first.
/// sipx takes the first `a=crypto` it can perform, which need not be the peer's first choice.
#[test]
fn the_answer_echoes_the_tag_of_the_suite_it_actually_accepted() {
    let their_key = Crypto::offer(2, Suite::AesCm128HmacSha1_80, true).expect("secure");
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
    assert_eq!(
        answered.media[0]
            .crypto()
            .expect("the answer carries a key")
            .tag,
        2,
        "tag 1 named a suite sipx cannot perform; the accepted attribute is tag 2"
    );
}

/// RFC 4568 §5.1.3: the offerer "MUST verify that one of the initially offered crypto suites and
/// its accompanying tag were accepted and echoed in the answer … If any of the above fails, the
/// negotiation MUST fail."
///
/// **The failing-first test for this story.** Before it, nothing compared the tags at all: an
/// answer naming a tag this side never sent was paired with our key and the call went ahead on
/// keys neither end agreed on.
#[test]
fn an_answer_whose_tag_was_never_offered_is_refused() {
    let ours = a_key();
    let theirs = Crypto::offer(7, Suite::AesCm128HmacSha1_80, true).expect("secure");

    let refused = Crypto::verify_answer(std::slice::from_ref(&ours), Some(&theirs));
    let error = refused.expect_err("tag 7 was never offered");
    let said = error.to_string();
    assert!(said.contains('7'), "the error says which tag: {said}");
    assert!(
        !said.contains(&theirs.to_value()),
        "and never the key material: {said}"
    );
}

/// The other half: an answer that echoes the tag is accepted, and what comes back is the offered
/// attribute it accepted — so the caller keys with *that* one rather than with any it sent.
#[test]
fn an_answer_that_echoes_an_offered_tag_is_accepted() {
    let ours = Crypto::offer(4, Suite::AesCm128HmacSha1_80, true).expect("secure");
    let theirs = Crypto::offer(4, Suite::AesCm128HmacSha1_80, true).expect("secure");

    let accepted = Crypto::verify_answer(std::slice::from_ref(&ours), Some(&theirs))
        .expect("tag 4 was offered");
    assert_eq!(
        accepted.key_and_salt, ours.key_and_salt,
        "our half of the keying"
    );
}

/// An answer naming a suite that was never offered is refused. It reaches this side as an
/// `a=crypto` carrying nothing sipx can perform, so the answer has no usable attribute at all —
/// which §5.1.3 makes a negotiation failure, not a call placed in the clear.
#[test]
fn an_answer_naming_a_suite_that_was_never_offered_is_refused() {
    let answered = parse(&offer(
        "RTP/SAVP",
        &["crypto:1 AES_256_CM_HMAC_SHA1_80 inline:AAAAAAAA".to_owned()],
    ))
    .expect("parses");
    let theirs = answered.media[0].crypto();
    assert!(theirs.is_none(), "sipx cannot perform AES_256_CM");

    assert!(
        Crypto::verify_answer(&[a_key()], theirs.as_ref()).is_err(),
        "an answer with nothing this side can key is a failed negotiation, not a plain call"
    );
}

/// And an answer that echoes the tag but carries no key. RFC 4568 §5.1.3 requires the offerer to
/// verify "that the answer contains a key" as well as the tag: half a keying is a stream that
/// connects and carries silence.
#[test]
fn an_answer_that_echoes_the_tag_but_carries_no_key_is_refused() {
    let ours = a_key();
    let keyless = Crypto {
        tag: ours.tag,
        suite: ours.suite,
        key_and_salt: Vec::new(),
    };
    assert!(Crypto::verify_answer(std::slice::from_ref(&ours), Some(&keyless)).is_err());
}

/// RFC 7714 §14.1 registers `AEAD_AES_128_GCM` and `AEAD_AES_256_GCM` in RFC 4568's crypto-suite
/// subregistry, with 16 + 12 and 32 + 12 octets of master key and salt respectively (§12, Tables 2
/// and 3). A peer that offers only these is a peer sipx could not talk to at all.
///
/// Asserted through `as_str` and the decoded lengths rather than through a variant name, so the
/// same test is legible before and after the suites exist.
#[test]
fn the_aead_suites_rfc7714_registers_are_read() {
    for (token, key_len, salt_len, inline) in [
        (
            "AEAD_AES_128_GCM",
            16,
            12,
            "AAECAwQFBgcICQoLDA0ODyAhIiMkJSYnKCkqKw==",
        ),
        (
            "AEAD_AES_256_GCM",
            32,
            12,
            "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh9AQUJDREVGR0hJSks=",
        ),
    ] {
        let parsed = Crypto::parse(&format!("1 {token} inline:{inline}"))
            .unwrap_or_else(|| panic!("RFC 7714 §14.1 registers {token}"));
        assert_eq!(parsed.suite.as_str(), token);
        assert_eq!(parsed.master_key().len(), key_len, "{token} master key");
        assert_eq!(parsed.master_salt().len(), salt_len, "{token} master salt");
    }
}

/// Selection is **by strength, never by peer order** — the rule `sipx_sip::auth::strongest`
/// applies to digest algorithms, for the same reason. An `a=crypto` list is not integrity
/// protected before the media is keyed, so an on-path attacker who reorders the lines picks the
/// cipher; ranking by strength removes the lever.
///
/// The counter-mode suite is listed first here and must not win.
#[test]
fn a_weaker_suite_offered_first_does_not_win() {
    let offered = parse(&offer(
        "RTP/SAVP",
        &[
            "crypto:1 AES_CM_128_HMAC_SHA1_80 \
             inline:d0RmdmcmVCspeEc3QGZiNWpVLFJhQX1cfHAwJSoj"
                .to_owned(),
            "crypto:2 AEAD_AES_128_GCM inline:AAECAwQFBgcICQoLDA0ODyAhIiMkJSYnKCkqKw==".to_owned(),
            "crypto:3 AEAD_AES_256_GCM \
             inline:AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh9AQUJDREVGR0hJSks="
                .to_owned(),
        ],
    ))
    .expect("parses");

    let chosen = offered.media[0]
        .crypto()
        .expect("a suite this side can perform");
    assert_eq!(
        chosen.suite.as_str(),
        "AEAD_AES_256_GCM",
        "the strongest suite offered must win regardless of the order the peer listed them in"
    );
    assert_eq!(chosen.tag, 3, "and its own tag travels back with it");
}
