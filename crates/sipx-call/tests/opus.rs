//! A call that selects Opus offers payload type 111 and carries Opus packets (`M-30`).
//!
//! `M-13` built the encoder, the decoder and the SDP half; this is the selection. The codec is
//! behind the `opus` feature, so the whole file is too: without the feature there is no Opus to
//! select, and what the *default* build promises — that no offer names Opus, and that an offer of
//! it is answered G.711 rather than refused — is asserted by `call.rs`'s own test module, which
//! runs in both builds.

#![cfg(feature = "opus")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
// `caller` and `callee` differ by two letters and are the names the RFCs, the industry and
// everyone reading this test already use. Renaming them to satisfy a similarity heuristic
// would make the test harder to read, not easier.
#![allow(clippy::similar_names)]

use std::net::IpAddr;
use std::time::Duration;

use sipx_call::{Call, Codecs, DialOptions, answer_with, dial};
use sipx_media::Codec;
use sipx_sip::{Host, HostName, Method, Uri};
use sipx_transport::{Config, Handle, Incoming, Target, bind};
use tokio::sync::mpsc::Receiver;

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("valid")
}

/// How long a test here waits for audio it played to arrive before calling it lost (`X-28`).
/// A bound on failure, not a window to measure in — see `MediaSession::record_at_least`.
const DELIVERY_BOUND: Duration = Duration::from_secs(10);

/// Opus packets carry 20 ms at the codec's fixed 48 kHz clock (RFC 7587 §7).
const SAMPLES_PER_PACKET: usize = 960;

async fn endpoint() -> (Handle, Receiver<Incoming>) {
    bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds")
}

/// A recognisable wideband clip: a 440 Hz tone at Opus's 48 kHz with an envelope, so a test
/// that silently recorded silence could not pass.
fn clip(milliseconds: usize) -> Vec<i16> {
    let samples = milliseconds * 48;
    (0..samples)
        .map(|i| {
            let t = f64::from(u32::try_from(i).unwrap_or(0)) / 48000.0;
            let envelope = (t * 3.0).min(1.0);
            let value = (t * 440.0 * 2.0 * std::f64::consts::PI).sin() * 12000.0 * envelope;
            i16::try_from(value.round() as i32).unwrap_or(0)
        })
        .collect()
}

/// Set up a caller and a callee that both select Opus, connect them, and hand back both sides
/// of the call plus the SDP the INVITE carried.
async fn connected() -> (Call, Call, String) {
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, _caller_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    let answering = tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("an INVITE arrives");
        assert_eq!(incoming.request.method, Method::Invite);
        let offered = String::from_utf8_lossy(incoming.request.body()).into_owned();
        let call = answer_with(&callee_endpoint, &incoming, loopback(), Codecs::Opus)
            .await
            .expect("answers");
        (call, offered)
    });

    let to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
    let caller = dial(
        &caller_endpoint,
        Target::udp(callee_addr),
        &to,
        &DialOptions::new("<sip:caller@example.net>", loopback()).with_codecs(Codecs::Opus),
    )
    .await
    .expect("the call connects");

    let (callee, offered) = answering.await.expect("the answering side finishes");
    (caller, callee, offered)
}

/// The failing-first test for `M-30`. It cannot pass without a selector: there is no
/// `Codecs` to pass, no `with_codecs` to pass it through, and no `answer_with` to answer with.
#[tokio::test]
async fn a_call_with_opus_selected_offers_111_and_carries_opus() {
    let (caller, callee, offered) = connected().await;

    // What went on the wire: Opus first, on the conventional dynamic type, with the rtpmap
    // that gives the number its meaning — a dynamic payload type means whatever `a=rtpmap`
    // said, and 111 without this line would mean nothing at all.
    let audio = offered
        .lines()
        .find(|line| line.starts_with("m=audio"))
        .expect("the offer describes an audio stream");
    assert!(
        audio.split_whitespace().any(|format| format == "111"),
        "payload type 111 is offered: {audio}"
    );
    assert!(
        offered.contains("a=rtpmap:111 opus/48000/2"),
        "the rtpmap says what 111 is:\n{offered}"
    );

    // Both ends settled on Opus — the answer honoured the offerer's first choice rather than
    // falling back to the G.711 the offer also carried.
    assert_eq!(caller.media().codec(), Codec::Opus, "the caller settled");
    assert_eq!(callee.media().codec(), Codec::Opus, "the callee settled");

    // And packets actually flow in it. The count is the proof: a G.711 decode of an Opus
    // packet yields a sample per byte — tens, not the 960 a 20 ms Opus frame carries — so
    // every sample arriving is only possible if both ends encoded and decoded Opus.
    let played = clip(300);
    let recorded = tokio::join!(
        async {
            caller.media().play(&played, SAMPLES_PER_PACKET).await;
        },
        async {
            callee
                .media()
                .record_at_least(played.len(), DELIVERY_BOUND)
                .await
        }
    )
    .1;

    assert!(!recorded.is_empty(), "the callee heard nothing at all");
    assert_eq!(recorded.len(), played.len(), "every sample arrived");
    assert!(
        recorded.iter().any(|&sample| sample != 0),
        "the callee heard silence, not the tone"
    );
}

/// The default does not move. G.711 is mandatory-to-implement and links no C library; Opus
/// does, so it is a choice the application makes — never one a build makes for it.
#[tokio::test]
async fn a_call_without_a_selection_still_offers_g711_only() {
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, _caller_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    let answering = tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("an INVITE arrives");
        let offered = String::from_utf8_lossy(incoming.request.body()).into_owned();
        let call = sipx_call::answer(&callee_endpoint, &incoming, loopback())
            .await
            .expect("answers");
        (call, offered)
    });

    let to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
    let caller = dial(
        &caller_endpoint,
        Target::udp(callee_addr),
        &to,
        &DialOptions::new("<sip:caller@example.net>", loopback()),
    )
    .await
    .expect("the call connects");
    let (callee, offered) = answering.await.expect("the answering side finishes");

    assert!(
        !offered.contains("opus"),
        "a default offer names no Opus:\n{offered}"
    );
    assert_eq!(caller.media().codec(), Codec::Pcmu);
    assert_eq!(callee.media().codec(), Codec::Pcmu);
}

