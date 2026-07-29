//! A call, against an implementation that did not write sipx's offer or its answer.
//!
//! `X-17`. Every other offer/answer test in this repo has sipx on both sides: the offer sipx
//! builds is read by the answerer sipx wrote, so two sides that misread the same sentence of
//! RFC 3264 agree perfectly and interoperate with nothing. These tests put a foreign user agent
//! on the far end of `M-1`'s pure function for the first time.
//!
//! They live beside the command line tool's tests because this crate is the one that already
//! depends on the whole stack — signalling, media and audio — which is exactly what a call
//! needs. Nothing here calls into the binary; the library is the thing under test.
//!
//! The media these carry is *cleartext*. The encrypted counterpart is `interop_srtp.rs`, added
//! by `X-27`; the clip, the round trip and the audio assertion are shared with it through
//! `interop_media`, so the two cannot come to mean different things.
//!
//! `#[ignore]`d, like the rest of the interop suite: a plain `cargo test` needs no containers.
//! `tests/interop/run.sh --peer asterisk` starts a peer and runs them.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
// `caller` and `callee` differ by two letters and are the words this domain uses.
#![allow(clippy::similar_names)]

mod interop_media;

use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use sipx_call::{DialOptions, answer, dial};
use sipx_sip::{Method, Uri};
use sipx_transport::{Config as TransportConfig, Target, bind};

use interop_media::{addr_in, assert_echo, echo_round_trip, echo_uri, loopback};

/// The port the peer has been configured to call, for the direction where sipx answers.
///
/// Fixed rather than chosen by the test: a peer's static contact is written before the test
/// runs, and it cannot be told a port that did not exist yet.
fn answering_port() -> u16 {
    std::env::var("SIPX_INTEROP_UA_PORT")
        .ok()
        .and_then(|port| port.parse().ok())
        .unwrap_or(5080)
}

/// Ask the peer to place a call towards sipx.
///
/// Shelling out to the container is deliberate. The alternative is a second SIP client in the
/// harness to trigger the first one, and a trigger that speaks SIP is a trigger that can be
/// wrong about SIP — which is the one thing this test must not have on the peer's side.
fn peer_originates() -> std::process::Output {
    let container = std::env::var("SIPX_INTEROP_CONTAINER").unwrap_or_else(|_| {
        panic!("SIPX_INTEROP_CONTAINER is unset; run this through tests/interop/run.sh")
    });
    let channel =
        std::env::var("SIPX_INTEROP_ORIGINATE").unwrap_or_else(|_| "PJSIP/sipx-ua".to_owned());
    std::process::Command::new("docker")
        .args([
            "exec",
            &container,
            "asterisk",
            "-rx",
            &format!("channel originate {channel} application Echo"),
        ])
        .output()
        .expect("docker exec runs")
}

/// `X-17`'s exit criterion, and the one this repository has never had: an implementation that
/// did not learn SIP from sipx answers a call sipx placed, negotiates a codec with it, carries
/// audio, and is hung up by a BYE sipx sends.
#[tokio::test]
#[ignore = "needs a user agent peer; see tests/interop/README.md"]
async fn an_independent_user_agent_answers_a_call_sipx_placed() {
    let mut config = TransportConfig::new("127.0.0.1:0".parse().expect("valid"));
    config.sent_by = loopback().to_string();
    let (handle, _incoming) = bind(config).await.expect("binds");

    let to = Uri::parse(Bytes::from(echo_uri())).expect("a SIP URI");
    let options =
        DialOptions::new("<sip:sipx@127.0.0.1>", loopback()).with_timeout(Duration::from_secs(15));

    let mut call = tokio::time::timeout(
        Duration::from_secs(20),
        dial(&handle, Target::udp(addr_in(&echo_uri())), &to, &options),
    )
    .await
    .expect("the peer answers rather than leaving us ringing")
    .expect("the peer accepts the call");

    // RFC 3264: the answer picked something out of the offer, and both ends agree on what.
    let codec = call.media().codec();
    assert_eq!(
        codec.payload_type(),
        0,
        "the negotiation chose {codec:?}; the offer's first and the peer's configured codec is µ-law"
    );

    // The media assertion proper: what the peer echoes back is what sipx sent it, on the
    // payload type the negotiation chose.
    let (sent, echoed) = echo_round_trip(&call).await;
    assert_echo(&sent, &echoed, codec.payload_type());

    call.hang_up().await.expect("the BYE is accepted");
    assert!(call.is_ended(), "the call is over on our side too");
}

/// The other direction, which exercises the other half of RFC 3264: sipx reads a foreign offer
/// and writes the answer, rather than writing the offer and reading a foreign answer.
#[tokio::test]
#[ignore = "needs a user agent peer; see tests/interop/README.md"]
async fn an_independent_user_agent_places_a_call_sipx_answers() {
    let local: SocketAddr = format!("127.0.0.1:{}", answering_port())
        .parse()
        .expect("valid");
    let mut config = TransportConfig::new(local);
    config.sent_by = loopback().to_string();
    let (handle, mut incoming) = bind(config)
        .await
        .unwrap_or_else(|e| panic!("binding {local}: {e}; the peer's contact names this port"));

    // Started only once sipx is listening, so a call that arrives is a call this test can take.
    let originate = std::thread::spawn(peer_originates);

    let request = tokio::time::timeout(Duration::from_secs(20), incoming.recv())
        .await
        .expect("the peer places the call within twenty seconds")
        .expect("an incoming request");
    assert_eq!(request.request.method, Method::Invite);

    let mut call = answer(&handle, &request, loopback())
        .await
        .expect("sipx answers the foreign offer");

    let codec = call.media().codec();
    assert_eq!(
        codec.payload_type(),
        0,
        "sipx answered a foreign offer with {codec:?}"
    );

    let (sent, echoed) = echo_round_trip(&call).await;
    assert_echo(&sent, &echoed, codec.payload_type());

    call.hang_up().await.expect("the BYE is accepted");

    let output = originate.join().expect("the originate command finishes");
    assert!(
        output.status.success(),
        "the peer refused to place the call: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
