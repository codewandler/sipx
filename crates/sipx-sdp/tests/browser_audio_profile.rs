//! M-49's failing-first pure SDP proofs, derived byte-for-byte from
//! `docs/specs/webrtc-audio.md` §9.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::IpAddr;

use sha2::{Digest as _, Sha256};
use sipx_sdp::browser_audio::{
    BrowserAudioLocal, BrowserAudioRole, IceChange, ProfileError, answer, offer, validate,
    validate_answer, validate_reoffer,
};
use sipx_sdp::fingerprint::{Fingerprint, SetupCapabilities};
use sipx_sdp::ice::{Candidate, Credentials};
use sipx_sdp::{Direction, parse};

const O1: &str = "v=0\r\n\
o=- 496232 1 IN IP4 192.0.2.10\r\n\
s=-\r\n\
t=0 0\r\n\
a=ice-options:ice2\r\n\
m=audio 49170 UDP/TLS/RTP/SAVPF 111 0 8 13 101\r\n\
c=IN IP4 192.0.2.10\r\n\
a=sendrecv\r\n\
a=rtcp-mux\r\n\
a=ice-ufrag:ofr1\r\n\
a=ice-pwd:offerPassword0123456789AB\r\n\
a=candidate:1 1 UDP 2130706431 192.0.2.10 49170 typ host\r\n\
a=fingerprint:sha-256 00:01:02:03:04:05:06:07:08:09:0A:0B:0C:0D:0E:0F:10:11:12:13:14:15:16:17:18:19:1A:1B:1C:1D:1E:1F\r\n\
a=setup:actpass\r\n\
a=rtpmap:111 opus/48000/2\r\n\
a=rtpmap:0 PCMU/8000\r\n\
a=rtpmap:8 PCMA/8000\r\n\
a=rtpmap:13 CN/8000\r\n\
a=rtpmap:101 telephone-event/8000\r\n\
a=fmtp:101 0-16\r\n";

const O2: &str = "v=0\r\n\
o=- 496232 1 IN IP4 192.0.2.10\r\n\
s=-\r\n\
t=0 0\r\n\
a=ice-options:ice2\r\n\
m=audio 49170 UDP/TLS/RTP/SAVPF 111 0 8 13 101\r\n\
c=IN IP4 192.0.2.10\r\n\
a=sendrecv\r\n\
a=rtcp-mux\r\n\
a=ice-ufrag:ofr1\r\n\
a=ice-pwd:offerPassword0123456789AB\r\n\
a=candidate:1 1 UDP 2130706431 192.0.2.10 49170 typ host\r\n\
a=candidate:2 2 UDP 2130706430 192.0.2.10 49171 typ host\r\n\
a=fingerprint:sha-256 00:01:02:03:04:05:06:07:08:09:0A:0B:0C:0D:0E:0F:10:11:12:13:14:15:16:17:18:19:1A:1B:1C:1D:1E:1F\r\n\
a=setup:actpass\r\n\
a=rtpmap:111 opus/48000/2\r\n\
a=rtpmap:0 PCMU/8000\r\n\
a=rtpmap:8 PCMA/8000\r\n\
a=rtpmap:13 CN/8000\r\n\
a=rtpmap:101 telephone-event/8000\r\n\
a=fmtp:101 0-16\r\n";

const A1: &str = "v=0\r\n\
o=- 772211 1 IN IP4 198.51.100.20\r\n\
s=-\r\n\
t=0 0\r\n\
a=ice-options:ice2\r\n\
m=audio 53000 UDP/TLS/RTP/SAVPF 111 0 8 13 101\r\n\
c=IN IP4 198.51.100.20\r\n\
a=sendrecv\r\n\
a=rtcp-mux\r\n\
a=ice-ufrag:ans1\r\n\
a=ice-pwd:answerPassword0123456789A\r\n\
a=candidate:1 1 UDP 2130706431 198.51.100.20 53000 typ host\r\n\
a=fingerprint:sha-256 20:21:22:23:24:25:26:27:28:29:2A:2B:2C:2D:2E:2F:30:31:32:33:34:35:36:37:38:39:3A:3B:3C:3D:3E:3F\r\n\
a=setup:active\r\n\
a=rtpmap:111 opus/48000/2\r\n\
a=rtpmap:0 PCMU/8000\r\n\
a=rtpmap:8 PCMA/8000\r\n\
a=rtpmap:13 CN/8000\r\n\
a=rtpmap:101 telephone-event/8000\r\n\
a=fmtp:101 0-16\r\n";

// Captured after a native headless browser reached `iceGatheringState === "complete"`. Volatile
// identifiers are evidence, not inputs to the policy; the shape is intentionally left intact.
const NATIVE_BROWSER_OFFER: &str = "v=0\r\n\
o=- 6190024055914035375 2 IN IP4 127.0.0.1\r\n\
s=-\r\n\
t=0 0\r\n\
a=group:BUNDLE 0\r\n\
a=extmap-allow-mixed\r\n\
a=msid-semantic: WMS\r\n\
m=audio 52175 UDP/TLS/RTP/SAVPF 111 63 9 0 8 13 110 126\r\n\
c=IN IP4 192.168.68.52\r\n\
a=rtcp:9 IN IP4 0.0.0.0\r\n\
a=candidate:3370245473 1 udp 2113937151 192.168.68.52 52175 typ host generation 0 network-cost 999\r\n\
a=ice-ufrag:Oxrs\r\n\
a=ice-pwd:1FMgxGqFxm0ynDDjASZyytlm\r\n\
a=ice-options:trickle\r\n\
a=fingerprint:sha-256 86:9C:49:68:4F:32:C7:67:61:B5:F7:C1:12:5F:8E:30:24:6A:2A:50:2B:1C:C1:2C:6B:3B:CF:43:03:B1:2E:E5\r\n\
a=setup:actpass\r\n\
a=mid:0\r\n\
a=extmap:1 urn:ietf:params:rtp-hdrext:ssrc-audio-level\r\n\
a=extmap:2 http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time\r\n\
a=extmap:3 http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01\r\n\
a=extmap:4 urn:ietf:params:rtp-hdrext:sdes:mid\r\n\
a=sendrecv\r\n\
a=msid:- 26756219-a927-4fa5-8e3d-ba8c62bf5ef3\r\n\
a=rtcp-mux\r\n\
a=rtcp-rsize\r\n\
a=rtpmap:111 opus/48000/2\r\n\
a=rtcp-fb:111 transport-cc\r\n\
a=fmtp:111 minptime=10;useinbandfec=1\r\n\
a=rtpmap:63 red/48000/2\r\n\
a=fmtp:63 111/111\r\n\
a=rtpmap:9 G722/8000\r\n\
a=rtpmap:0 PCMU/8000\r\n\
a=rtpmap:8 PCMA/8000\r\n\
a=rtpmap:13 CN/8000\r\n\
a=rtpmap:110 telephone-event/48000\r\n\
a=rtpmap:126 telephone-event/8000\r\n\
a=ssrc:2005259182 cname:xhHzorYIekgQwPXO\r\n\
a=ssrc:2005259182 msid:- 26756219-a927-4fa5-8e3d-ba8c62bf5ef3\r\n";

fn local(
    address: &str,
    port: u16,
    session_id: u64,
    ufrag: &str,
    pwd: &str,
    candidate: &str,
    fingerprint: &str,
) -> BrowserAudioLocal {
    BrowserAudioLocal {
        address: address.parse::<IpAddr>().expect("literal IP"),
        port,
        session_id,
        session_version: 1,
        direction: Direction::SendRecv,
        ice: Credentials::new(ufrag, pwd).expect("vector credentials"),
        candidates: vec![Candidate::parse(candidate).expect("vector candidate")],
        fingerprint: Fingerprint::parse(fingerprint).expect("vector fingerprint"),
        setup: SetupCapabilities::both(),
    }
}

fn offer_local() -> BrowserAudioLocal {
    local(
        "192.0.2.10",
        49_170,
        496_232,
        "ofr1",
        "offerPassword0123456789AB",
        "1 1 UDP 2130706431 192.0.2.10 49170 typ host",
        "sha-256 00:01:02:03:04:05:06:07:08:09:0A:0B:0C:0D:0E:0F:10:11:12:13:14:15:16:17:18:19:1A:1B:1C:1D:1E:1F",
    )
}

fn answer_local() -> BrowserAudioLocal {
    local(
        "198.51.100.20",
        53_000,
        772_211,
        "ans1",
        "answerPassword0123456789A",
        "1 1 UDP 2130706431 198.51.100.20 53000 typ host",
        "sha-256 20:21:22:23:24:25:26:27:28:29:2A:2B:2C:2D:2E:2F:30:31:32:33:34:35:36:37:38:39:3A:3B:3C:3D:3E:3F",
    )
}

/// `BA-SDP-O1` and `BA-SDP-A1`: the fixture bytes are the normative bytes, not a similar SDP.
#[test]
fn complete_vectors_have_the_normative_identity_and_validate() {
    assert_eq!(O1.len(), 555);
    assert_eq!(A1.len(), 563);
    assert_eq!(
        format!("{:x}", Sha256::digest(O1)),
        "44fd3d3cc886a667f3b89d50c5bb7453ce985d24851252660c25c8399ae12c25"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(A1)),
        "518f6918170dc6bd118b653df7db3d4a4136f94cd38c973c6ee5f49784c0343e"
    );

    let offered = parse(O1).expect("O1 parses");
    let answered = parse(A1).expect("A1 parses");
    let offer_profile = validate(&offered, BrowserAudioRole::Offerer).expect("O1 profile");
    let answer_profile =
        validate_answer(&offered, &answered, SetupCapabilities::both()).expect("A1 profile");

    assert_eq!(offer_profile.payloads.opus, 111);
    assert_eq!(answer_profile.description.payloads.opus, 111);
    assert_eq!(
        answer_profile.local_setup,
        sipx_sdp::fingerprint::Setup::Passive
    );
}

/// `BA-SDP-O2`: an initial mux offer may retain a bounded separate-port fallback. The answerer
/// discards its component-two candidate before it constructs the one-component ICE description.
#[test]
fn an_unused_rtcp_candidate_in_a_mux_offer_is_not_a_runtime_component() {
    assert_eq!(O2.len(), 613);
    assert_eq!(
        format!("{:x}", Sha256::digest(O2)),
        "5957da5732ebe747cffa9cc940381eb63caf7c2bfc0dca4beb7f4609e178e4d0"
    );
    let offered = parse(O2).expect("O2 parses");
    let profile = validate(&offered, BrowserAudioRole::Offerer).expect("O2 profile");
    assert_eq!(profile.candidates.len(), 1);
    assert_eq!(
        profile.candidates[0].component,
        sipx_sdp::ice::ComponentId::RTP
    );

    let answered = answer(&offered, &answer_local()).expect("O2 is answerable");
    assert!(
        answered.media[0]
            .ice_candidates()
            .iter()
            .all(|candidate| candidate.component == sipx_sdp::ice::ComponentId::RTP)
    );
}

/// The ignored fallback cannot substitute for the profile's media component, remove mux, or grow
/// without a bound. A component-two line in an answer remains a contradiction after mux won.
#[test]
fn unused_rtcp_candidates_do_not_weaken_the_profile_boundary() {
    let no_component_one = parse(&O2.replace(
        "a=candidate:1 1 UDP 2130706431 192.0.2.10 49170 typ host\r\n",
        "",
    ))
    .expect("component-two-only offer parses");
    assert_eq!(
        validate(&no_component_one, BrowserAudioRole::Offerer),
        Err(ProfileError::IceRequired)
    );

    let no_mux = parse(&O2.replace("a=rtcp-mux\r\n", "")).expect("non-mux offer parses");
    assert_eq!(
        validate(&no_mux, BrowserAudioRole::Offerer),
        Err(ProfileError::RtcpMuxRequired)
    );

    let extra = "a=candidate:2 2 UDP 2130706430 192.0.2.10 49171 typ host\r\n";
    let over_bound = parse(&O1.replace(
        "a=setup:actpass\r\n",
        &format!("{}a=setup:actpass\r\n", extra.repeat(33)),
    ))
    .expect("bounded fallback offer parses");
    assert_eq!(
        validate(&over_bound, BrowserAudioRole::Offerer),
        Err(ProfileError::IceRequired)
    );

    let answer_with_component_two = answer_mutation(
        "a=candidate:1 1 UDP 2130706431 198.51.100.20 53000 typ host\r\n",
        "a=candidate:1 1 UDP 2130706431 198.51.100.20 53000 typ host\r\n\
         a=candidate:2 2 UDP 2130706430 198.51.100.20 53001 typ host\r\n",
    );
    assert_eq!(
        validate(&answer_with_component_two, BrowserAudioRole::Answerer),
        Err(ProfileError::RtcpMuxRequired)
    );
}

/// The generators consume all local facts and reproduce the normative complete vectors exactly.
#[test]
fn complete_offer_and_answer_generation_is_byte_exact() {
    let generated_offer = offer(&offer_local()).expect("complete offer");
    assert_eq!(generated_offer.to_string_sdp(), O1);

    let generated_answer = answer(&generated_offer, &answer_local()).expect("complete answer");
    assert_eq!(generated_answer.to_string_sdp(), A1);
}

/// A completed native-browser offer may advertise trickle capability, its conventional muxed
/// RTCP placeholder, and safe formats beyond the five this profile implements.
#[test]
fn completed_native_browser_shape_validates_and_answers_with_the_required_intersection() {
    assert_eq!(NATIVE_BROWSER_OFFER.len(), 1_298);
    assert_eq!(
        format!("{:x}", Sha256::digest(NATIVE_BROWSER_OFFER)),
        "451fd0acdd766200f1f5b711d92cac518f7242558ff722b1cb440d544f47c75f"
    );
    let offered = parse(NATIVE_BROWSER_OFFER).expect("native offer parses");
    let profile = validate(&offered, BrowserAudioRole::Offerer).expect("native offer validates");
    assert_eq!(profile.payloads.opus, 111);
    assert_eq!(profile.payloads.telephone_event, 126);
    assert_eq!(profile.candidates.len(), 1);

    let answered = answer(&offered, &answer_local()).expect("native offer is answerable");
    assert_eq!(answered.media[0].formats, ["111", "0", "8", "13", "126"]);
    assert_eq!(
        answered.media[0].rtpmap("126"),
        Some("telephone-event/8000")
    );
    validate_answer(&offered, &answered, SetupCapabilities::both())
        .expect("generated required intersection validates");
}

/// A native browser answering O1 retains all five mappings but may advertise trickle, carry the
/// muxed RTCP placeholder, and rely on RFC 4733's absent-fmtp DTMF default.
#[test]
fn native_browser_answer_shape_accepts_the_telephone_event_default() {
    let offered = parse(O1).expect("O1");
    let native_answer = A1
        .replace("a=ice-options:ice2", "a=ice-options:trickle")
        .replace(
            "c=IN IP4 198.51.100.20\r\n",
            "c=IN IP4 198.51.100.20\r\na=rtcp:9 IN IP4 0.0.0.0\r\n",
        )
        .replace("a=fmtp:101 0-16\r\n", "");
    let native_answer = parse(&native_answer).expect("native answer parses");
    validate_answer(&offered, &native_answer, SetupCapabilities::both())
        .expect("native answer defaults to DTMF events 0-15");
}

fn answer_mutation(from: &str, to: &str) -> sipx_sdp::SessionDescription {
    parse(&A1.replacen(from, to, 1)).expect("the mutation remains SDP")
}

/// `BA-SDP-N1` through `N9`: every mandatory boundary has its own fail-closed result.
#[test]
fn negative_answer_vectors_fail_at_the_named_boundary() {
    let offered = parse(O1).expect("O1");
    let cases = [
        (
            answer_mutation("a=rtcp-mux\r\n", ""),
            ProfileError::RtcpMuxRequired,
        ),
        (
            answer_mutation("UDP/TLS/RTP/SAVPF", "RTP/SAVP"),
            ProfileError::WeakerMedia,
        ),
        (
            answer_mutation("a=setup:active", "a=setup:actpass"),
            ProfileError::SetupRole,
        ),
        (
            answer_mutation(
                "a=fingerprint:sha-256 20:21:22:23:24:25:26:27:28:29:2A:2B:2C:2D:2E:2F:30:31:32:33:34:35:36:37:38:39:3A:3B:3C:3D:3E:3F\r\n",
                "",
            ),
            ProfileError::FingerprintRequired,
        ),
        (
            answer_mutation(
                "a=candidate:1 1 UDP 2130706431 198.51.100.20 53000 typ host\r\n",
                "",
            ),
            ProfileError::IceRequired,
        ),
        (
            answer_mutation(
                "m=audio 53000 UDP/TLS/RTP/SAVPF 111 0 8 13 101\r\n",
                "m=audio 53000 UDP/TLS/RTP/SAVPF 0 8 13 101\r\n",
            )
            .tap_mut(|answer| {
                answer.media[0]
                    .attributes
                    .retain(|attribute| attribute.value.as_deref() != Some("111 opus/48000/2"));
            }),
            ProfileError::CodecSetIncomplete,
        ),
        (
            answer_mutation(
                "a=setup:active\r\n",
                "a=setup:active\r\na=crypto:1 AES_CM_128_HMAC_SHA1_80 inline:dGVzdA==\r\n",
            ),
            ProfileError::WeakerMedia,
        ),
        (
            answer_mutation(
                "a=fmtp:101 0-16\r\n",
                "a=fmtp:101 0-16\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n",
            ),
            ProfileError::MediaSectionCount,
        ),
        (
            answer_mutation(
                "a=candidate:1 1 UDP 2130706431 198.51.100.20 53000 typ host\r\n",
                "a=candidate:1 1 UDP 2130706431 198.51.100.20 53000 typ host\r\na=candidate:2 2 UDP 2130706430 198.51.100.20 53001 typ host\r\n",
            ),
            ProfileError::RtcpMuxRequired,
        ),
    ];

    for (answered, expected) in cases {
        assert_eq!(
            validate_answer(&offered, &answered, SetupCapabilities::both()),
            Err(expected)
        );
    }
}

trait TapMut: Sized {
    fn tap_mut(mut self, change: impl FnOnce(&mut Self)) -> Self {
        change(&mut self);
        self
    }
}

impl<T> TapMut for T {}

/// Subsequent descriptions repeat the profile. Both ICE credentials changing is a restart;
/// changing one or removing a mandatory element leaves the established generation untouched.
#[test]
fn reoffer_validation_distinguishes_preservation_restart_and_removal() {
    let current = parse(O1).expect("O1");
    assert_eq!(
        validate_reoffer(&current, &current, BrowserAudioRole::Offerer)
            .expect("unchanged profile")
            .ice_change,
        IceChange::Unchanged
    );

    let reordered =
        parse(&O1.replace("111 0 8 13 101", "0 111 8 13 101")).expect("reordered profile");
    assert_eq!(
        validate_reoffer(&current, &reordered, BrowserAudioRole::Offerer)
            .expect("mapping-preserving reorder")
            .ice_change,
        IceChange::Unchanged
    );

    let only_ufrag =
        parse(&O1.replace("a=ice-ufrag:ofr1", "a=ice-ufrag:new1")).expect("one-sided restart SDP");
    assert_eq!(
        validate_reoffer(&current, &only_ufrag, BrowserAudioRole::Offerer),
        Err(ProfileError::IceRequired)
    );

    let removed = parse(&O1.replace("a=rtcp-mux\r\n", "")).expect("removed mux SDP");
    assert_eq!(
        validate_reoffer(&current, &removed, BrowserAudioRole::Offerer),
        Err(ProfileError::ProfileRemoved)
    );

    let restarted = parse(&O1.replace("a=ice-ufrag:ofr1", "a=ice-ufrag:new1").replace(
        "a=ice-pwd:offerPassword0123456789AB",
        "a=ice-pwd:restartPassword012345678",
    ))
    .expect("complete restart SDP");
    assert_eq!(
        validate_reoffer(&current, &restarted, BrowserAudioRole::Offerer)
            .expect("complete restart")
            .ice_change,
        IceChange::Restart
    );
}

/// Browser audio narrows generic RFC 8839 candidate extensibility: a usable host line cannot hide
/// malformed input or a candidate kind this first profile does not implement.
#[test]
fn candidate_set_is_complete_and_limited_to_host_or_server_reflexive() {
    let offered = parse(O1).expect("O1");
    for extra in [
        "a=candidate:not a candidate\r\n",
        "a=candidate:relay 1 UDP 2130706430 198.51.100.30 53002 typ relay raddr 0.0.0.0 rport 9\r\n",
        "a=candidate:prflx 1 UDP 2130706429 198.51.100.31 53003 typ prflx raddr 0.0.0.0 rport 9\r\n",
    ] {
        let answer =
            parse(&A1.replace("a=setup:active\r\n", &format!("{extra}a=setup:active\r\n")))
                .expect("candidate mutation remains SDP");
        assert_eq!(
            validate_answer(&offered, &answer, SetupCapabilities::both()),
            Err(ProfileError::IceRequired),
            "extra line was accepted: {extra:?}"
        );
    }

    for local_candidate in [
        "relay 1 UDP 2130706430 192.0.2.10 49170 typ relay raddr 0.0.0.0 rport 9",
        "prflx 1 UDP 2130706429 192.0.2.10 49170 typ prflx raddr 0.0.0.0 rport 9",
    ] {
        let mut local = offer_local();
        local.candidates = vec![Candidate::parse(local_candidate).expect("typed candidate")];
        assert_eq!(
            offer(&local),
            Err(ProfileError::IceRequired),
            "local generation admitted {local_candidate:?}"
        );
    }
}
