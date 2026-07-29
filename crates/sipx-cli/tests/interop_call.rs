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

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use bytes::Bytes;
use sipx_audio::g711;
use sipx_call::{Call, DialOptions, answer, dial};
use sipx_sip::{Method, Uri};
use sipx_transport::{Config as TransportConfig, Target, bind};

/// Where the peer answers calls. A URI rather than an address: what makes a call reach a
/// dialplan is the user part, and which user is a property of the peer, not of the test.
fn echo_uri() -> String {
    std::env::var("SIPX_INTEROP_ECHO_URI").unwrap_or_else(|_| "sip:echo@127.0.0.1:5060".to_owned())
}

/// The address inside that URI, which is where the request is actually sent.
///
/// Parsed from the URI rather than taken from a second variable, so the two cannot disagree.
fn echo_addr() -> SocketAddr {
    let uri = echo_uri();
    let host = uri.rsplit('@').next().unwrap_or_default();
    host.parse().unwrap_or_else(|e| {
        panic!("{host}: not an address and port ({e}); check SIPX_INTEROP_ECHO_URI")
    })
}

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("valid")
}

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

/// A recognisable clip: a 440 Hz tone with an envelope, so a test that recorded silence could
/// not pass for one that heard the call.
fn tone(milliseconds: usize) -> Vec<i16> {
    let samples = milliseconds * 8;
    (0..samples)
        .map(|i| {
            let t = f64::from(u32::try_from(i).unwrap_or(0)) / 8000.0;
            let envelope = (t * 4.0).min(1.0);
            let value = (t * 440.0 * 2.0 * std::f64::consts::PI).sin() * 12000.0 * envelope;
            i16::try_from(value.round() as i32).unwrap_or(0)
        })
        .collect()
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

/// The longest run of bytes the two streams share, once aligned.
///
/// `M-3` compares the whole array, because there both ends are sipx and nothing is lost. Here
/// the far end is a foreign RTP stack that starts echoing before the last packet has been sent,
/// so the comparison is over the aligned overlap rather than the whole clip. Within that overlap
/// the claim is unchanged and it is the strong one: byte for byte, because µ-law in and µ-law
/// out means nothing on the path was entitled to transcode it.
fn longest_aligned_match(sent: &[u8], received: &[u8]) -> usize {
    let window = 320; // 40 ms of µ-law: long enough that a chance match is not plausible
    if sent.len() < window || received.len() < window {
        return 0;
    }
    let probe = &received[..window];
    let Some(offset) = sent.windows(window).position(|w| w == probe) else {
        return 0;
    };
    sent[offset..]
        .iter()
        .zip(received.iter())
        .take_while(|(a, b)| a == b)
        .count()
}

/// What came back off the wire, unpacked no further than the RTP header.
struct Echoed {
    /// The payload type every packet carried.
    payload_types: Vec<u8>,
    /// The payloads, concatenated.
    payload: Vec<u8>,
}

/// Play a tone into a connected call and collect what the far end sends back.
///
/// Deliberately in relay mode. The assertion this feeds is about the bytes an independent
/// implementation put on the wire, and a decode step on the way in would be sipx's opinion of
/// those bytes rather than the bytes.
async fn echo_round_trip(call: &Call) -> (Vec<u8>, Echoed) {
    let source = tone(600);
    let media = call.media();
    media.set_relay(true);

    let played = source.clone();
    let collected = tokio::join!(async { media.play(&played, 160).await }, async {
        let mut echoed = Echoed {
            payload_types: Vec::new(),
            payload: Vec::new(),
        };
        // Stop when the far end goes quiet, not after a fixed count: how many packets an echo
        // returns is the far end's business.
        while let Ok(Some(packet)) =
            tokio::time::timeout(Duration::from_millis(600), media.recv_encoded()).await
        {
            echoed.payload_types.push(packet.payload_type);
            echoed.payload.extend_from_slice(&packet.payload);
        }
        echoed
    })
    .1;

    media.set_relay(false);
    (g711::ulaw_encode_all(&source), collected)
}

/// The assertions both directions share: audio arrived, it arrived on the payload type the
/// negotiation chose, and it is the audio that was sent.
fn assert_echo(sent: &[u8], echoed: &Echoed, expected_payload_type: u8) {
    assert!(
        !echoed.payload.is_empty(),
        "the call connected and no audio came back; a session was set up and nothing was heard"
    );

    // The number in the answer is the number on the wire. A stack that negotiated one payload
    // type and sent another would still set up a session and still carry noise.
    let unexpected: Vec<u8> = echoed
        .payload_types
        .iter()
        .copied()
        .filter(|pt| *pt != expected_payload_type)
        .collect();
    assert!(
        unexpected.is_empty(),
        "the peer sent payload types {unexpected:?} where the negotiation chose \
         {expected_payload_type}"
    );

    // What is observed is the whole clip, byte for byte, in both directions — `M-3`'s
    // bit-exactness with a foreign implementation in the middle, relaxed not at all. The
    // allowance is three packets, so a datagram genuinely lost on a loaded runner is not
    // reported as an interop defect; it is nowhere near enough for a peer that resampled,
    // transcoded or substituted comfort noise to pass.
    let floor = sent.len().saturating_sub(3 * 160);
    let matched = longest_aligned_match(sent, &echoed.payload);
    assert!(
        matched >= floor,
        "only {matched} of the {} bytes sent came back unchanged; µ-law in and µ-law out \
         means nothing on the path should have transcoded it",
        sent.len()
    );
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
        dial(&handle, Target::udp(echo_addr()), &to, &options),
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
