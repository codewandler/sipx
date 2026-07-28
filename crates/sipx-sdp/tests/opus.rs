//! Negotiating Opus, which is the case the dynamic-payload-type rule from `M-1` exists for.
//!
//! A static payload type means the same thing everywhere: 0 is µ-law and always has been.
//! A dynamic one means whatever the `rtpmap` says, and the *number* carries no information at
//! all — two endpoints can and routinely do pick different numbers for the same codec. Matching
//! on the number is the mistake this file exists to catch.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fmt::Write as _;
use std::net::IpAddr;

use sipx_sdp::{Capabilities, answer, parse};

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("valid")
}

/// An offer with the given audio format list and rtpmaps.
fn offer(formats: &str, rtpmaps: &[&str]) -> String {
    let mut sdp = format!(
        "v=0\r\no=- 1 1 IN IP4 192.0.2.1\r\ns=-\r\nc=IN IP4 192.0.2.1\r\nt=0 0\r\n\
         m=audio 40000 RTP/AVP {formats}\r\n"
    );
    for rtpmap in rtpmaps {
        let _ = write!(sdp, "a=rtpmap:{rtpmap}\r\n");
    }
    sdp
}

fn formats_of(sdp: &sipx_sdp::SessionDescription) -> Vec<String> {
    sdp.media[0].formats.clone()
}

/// Opus offered on 111, answered by an endpoint that also calls it 111.
#[test]
fn opus_is_negotiated_by_its_encoding_name() {
    let offered = parse(&offer("111 0", &["111 opus/48000/2", "0 PCMU/8000"])).expect("parses");
    let answered = answer(&offered, &Capabilities::with_opus(loopback(), 40_002));
    assert!(
        formats_of(&answered).contains(&"111".to_owned()),
        "Opus must be in the answer: {:?}",
        formats_of(&answered)
    );
}

/// The case that separates matching by name from matching by number. The offerer calls Opus
/// **96**; sipx calls it 111. They are the same codec, and an implementation comparing numbers
/// would answer G.711 and lose the better codec for no reason.
#[test]
fn opus_is_matched_even_when_the_far_end_numbers_it_differently() {
    let offered = parse(&offer("96 0", &["96 opus/48000/2", "0 PCMU/8000"])).expect("parses");
    let answered = answer(&offered, &Capabilities::with_opus(loopback(), 40_002));

    let formats = formats_of(&answered);
    assert!(
        formats.contains(&"96".to_owned()),
        "the answer must use the *offerer's* number for the codec: {formats:?}"
    );
    assert!(
        !formats.contains(&"111".to_owned()),
        "answering with our own number would name a codec the offerer never offered: {formats:?}"
    );

    // And the rtpmap in the answer says what 96 is, using the offerer's spelling.
    let rtpmap = answered.media[0].rtpmap("96").expect("an rtpmap for 96");
    assert!(rtpmap.to_ascii_lowercase().starts_with("opus"), "{rtpmap}");
}

/// The offerer's order is kept. RFC 3264 §6.1: the offerer's first choice is what it most wants
/// used, and an answerer that reordered would be how two endpoints end up transcoding.
#[test]
fn the_offerers_preference_is_honoured_even_when_it_prefers_g711() {
    let offered = parse(&offer("0 111", &["0 PCMU/8000", "111 opus/48000/2"])).expect("parses");
    let answered = answer(&offered, &Capabilities::with_opus(loopback(), 40_002));

    let formats = formats_of(&answered);
    assert_eq!(
        formats.first().map(String::as_str),
        Some("0"),
        "the offerer asked for G.711 first: {formats:?}"
    );
}

/// G.711 stays the fallback. An endpoint that offered only Opus would fail to call most of the
/// telephone network.
#[test]
fn an_endpoint_with_no_opus_still_gets_g711() {
    let offered = parse(&offer("0 8", &["0 PCMU/8000", "8 PCMA/8000"])).expect("parses");
    let answered = answer(&offered, &Capabilities::with_opus(loopback(), 40_002));

    let formats = formats_of(&answered);
    assert!(formats.contains(&"0".to_owned()), "{formats:?}");
    assert_ne!(answered.media[0].port, 0, "the stream must not be rejected");
}

/// A dynamic payload type with no `rtpmap` is uninterpretable whatever the number, and must not
/// be guessed at from the number alone.
#[test]
fn a_dynamic_type_with_no_rtpmap_is_not_assumed_to_be_opus() {
    let offered = parse(&offer("111 0", &["0 PCMU/8000"])).expect("parses");
    let answered = answer(&offered, &Capabilities::with_opus(loopback(), 40_002));

    let formats = formats_of(&answered);
    assert!(
        !formats.contains(&"111".to_owned()),
        "111 means nothing without an rtpmap: {formats:?}"
    );
    assert!(formats.contains(&"0".to_owned()), "{formats:?}");
}

/// A different codec on a number sipx uses for Opus must not be taken for Opus.
#[test]
fn another_codec_on_our_opus_number_is_not_opus() {
    let offered = parse(&offer("111 0", &["111 G729/8000", "0 PCMU/8000"])).expect("parses");
    let answered = answer(&offered, &Capabilities::with_opus(loopback(), 40_002));

    let formats = formats_of(&answered);
    assert!(
        !formats.contains(&"111".to_owned()),
        "111 is G.729 here, whatever sipx calls 111: {formats:?}"
    );
}
