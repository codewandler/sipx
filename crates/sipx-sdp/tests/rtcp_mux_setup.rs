//! M-46's failing-first offer/answer proofs, derived from
//! `docs/specs/rtcp-mux-setup.md` §5.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::{IpAddr, Ipv4Addr};

use sipx_sdp::fingerprint::{Setup, SetupCapabilities, SetupRoleError};
use sipx_sdp::{Capabilities, RtcpMode, answer, parse};

fn loopback() -> IpAddr {
    IpAddr::V4(Ipv4Addr::LOCALHOST)
}

fn offer(extra: &str) -> sipx_sdp::SessionDescription {
    parse(&format!(
        "v=0\r\n\
         o=- 1 1 IN IP4 192.0.2.10\r\n\
         s=-\r\n\
         c=IN IP4 192.0.2.10\r\n\
         t=0 0\r\n\
         m=audio 49170 RTP/AVP 0\r\n\
         a=rtpmap:0 PCMU/8000\r\n\
         {extra}"
    ))
    .expect("the fixture is valid SDP")
}

/// `MUX-SDP-1`: an offered flag is answered and settles one-port operation.
#[test]
fn rtcp_mux_is_answered_only_when_both_sides_agree() {
    let offered = offer("a=rtcp-mux\r\n");
    let answered = answer(
        &offered,
        &Capabilities::g711(loopback(), 40_000).with_rtcp_mux(),
    );
    let offer_audio = offered.media.first().expect("offer audio");
    let answer_audio = answered.media.first().expect("answer audio");

    assert!(answer_audio.rtcp_mux(), "the answer carries a=rtcp-mux");
    assert_eq!(
        RtcpMode::from_exchange(offer_audio, answer_audio),
        RtcpMode::Mux
    );
}

/// `MUX-SDP-2`: omission in the answer is the RFC 5761 fallback, not a failed exchange.
#[test]
fn an_answer_omitting_rtcp_mux_settles_separate_ports() {
    let offered = offer("a=rtcp-mux\r\n");
    let answered = answer(&offered, &Capabilities::g711(loopback(), 40_000));
    let offer_audio = offered.media.first().expect("offer audio");
    let answer_audio = answered.media.first().expect("answer audio");

    assert!(!answer_audio.rtcp_mux());
    assert_eq!(
        RtcpMode::from_exchange(offer_audio, answer_audio),
        RtcpMode::Separate
    );
}

/// An answer never introduces mux when the offer did not ask for it.
#[test]
fn rtcp_mux_is_not_inserted_into_an_answer_unasked() {
    let offered = offer("");
    let answered = answer(
        &offered,
        &Capabilities::g711(loopback(), 40_000).with_rtcp_mux(),
    );

    assert!(!answered.media.first().expect("answer audio").rtcp_mux());
}

/// A rejected offer section cannot negotiate a running socket mode, even if both descriptions
/// retain the mux flag while preserving media-line alignment.
#[test]
fn a_rejected_offer_section_never_settles_mux() {
    let mut offered = offer("a=rtcp-mux\r\n");
    offered.media.first_mut().expect("offer audio").port = 0;
    let mut answered = answer(
        &offered,
        &Capabilities::g711(loopback(), 40_000).with_rtcp_mux(),
    );
    answered
        .media
        .first_mut()
        .expect("answer audio")
        .attributes
        .push(sipx_sdp::Attribute::flag("rtcp-mux"));

    assert_eq!(
        RtcpMode::from_exchange(
            offered.media.first().expect("offer audio"),
            answered.media.first().expect("answer audio")
        ),
        RtcpMode::Separate
    );
}

/// RFC 5761 §4 reserves payload types 64..=95 from a muxed stream so their marked form cannot be
/// mistaken for an RTCP packet type.
#[test]
fn a_muxed_answer_refuses_a_colliding_rtp_payload_type() {
    let offered = parse(
        "v=0\r\n\
         o=- 1 1 IN IP4 192.0.2.10\r\n\
         s=-\r\n\
         c=IN IP4 192.0.2.10\r\n\
         t=0 0\r\n\
         m=audio 49170 RTP/AVP 72\r\n\
         a=rtpmap:72 PCMU/8000\r\n\
         a=rtcp-mux\r\n",
    )
    .expect("valid SDP");
    let answered = answer(
        &offered,
        &Capabilities::g711(loopback(), 40_000).with_rtcp_mux(),
    );

    assert!(
        answered.media.first().expect("answer audio").is_rejected(),
        "the only common format collides with RTCP under mux"
    );
}

/// `SETUP-1` and `SETUP-2`: the offerer takes the role complementary to the answer.
#[test]
fn both_legal_dtls_answers_select_the_complementary_local_role() {
    let roles = SetupCapabilities::both();
    assert_eq!(
        roles.from_answer(Some(Setup::Active)),
        Ok(Setup::Passive),
        "an active answerer makes the offerer the DTLS server"
    );
    assert_eq!(
        roles.from_answer(Some(Setup::Passive)),
        Ok(Setup::Active),
        "a passive answerer makes the offerer the DTLS client"
    );
}

/// `SETUP-N1` and `SETUP-N2`: refusal is typed and precedes any handshake API.
#[test]
fn unresolved_or_unsupported_answer_roles_are_typed_errors() {
    assert_eq!(
        SetupCapabilities::both().from_answer(Some(Setup::ActPass)),
        Err(SetupRoleError::UnresolvedAnswer(Setup::ActPass))
    );
    assert_eq!(
        SetupCapabilities::client_only().from_answer(Some(Setup::Active)),
        Err(SetupRoleError::UnsupportedLocalRole(Setup::Passive))
    );
    assert_eq!(
        SetupCapabilities::both().from_answer(None),
        Err(SetupRoleError::MissingAnswer)
    );
}

/// An answerer prefers active but can select passive when only the server role exists.
#[test]
fn the_answer_role_is_selected_from_local_capabilities() {
    assert_eq!(
        SetupCapabilities::both().answer_to(Setup::ActPass),
        Ok(Setup::Active)
    );
    assert_eq!(
        SetupCapabilities::server_only().answer_to(Setup::ActPass),
        Ok(Setup::Passive)
    );
    assert_eq!(
        SetupCapabilities::neither().answer_to(Setup::ActPass),
        Err(SetupRoleError::NoAnswerRole)
    );
    assert_eq!(
        SetupCapabilities::both().answer_to(Setup::HoldConn),
        Err(SetupRoleError::UnresolvedOffer(Setup::HoldConn)),
        "DTLS-SRTP cannot send a successful answer for an offer that forbids a handshake"
    );
}

/// Runtime role selection and SDP answering share the same media-then-session resolver.
#[test]
fn a_session_level_setup_role_applies_to_the_audio_stream() {
    let description = offer("");
    let mut description = description;
    description
        .attributes
        .push(sipx_sdp::Attribute::valued("setup", "passive"));
    let audio = description.media.first().expect("audio");

    assert_eq!(
        sipx_sdp::answer::setup_of(&description, audio),
        Some(Setup::Passive)
    );
}
