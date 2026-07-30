//! What the interop call tests share: a clip, a round trip, and the assertion on the audio.
//!
//! Extracted by `X-27`, which added a second call test. The reason it is a module rather than a
//! second copy is the assertion at the bottom of this file: the encrypted call has to be held to
//! *the same* bit-exactness as the cleartext one, and two copies of that arithmetic drift — the
//! encrypted one being the one that would quietly get softened, since it is the one that fails
//! when something is wrong with the keying.

// Each test binary uses a subset of this, and the unused half is not dead code — it is code the
// other binary uses.
#![allow(dead_code)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use sipx_audio::g711;
use sipx_call::Call;

/// Where the peer answers calls. A URI rather than an address: what makes a call reach a
/// dialplan is the user part, and which user is a property of the peer, not of the test.
pub(crate) fn echo_uri() -> String {
    std::env::var("SIPX_INTEROP_ECHO_URI").unwrap_or_else(|_| "sip:echo@127.0.0.1:5060".to_owned())
}

/// The address inside a peer URI, which is where the request is actually sent.
///
/// Parsed from the URI rather than taken from a second variable, so the two cannot disagree.
pub(crate) fn addr_in(uri: &str) -> SocketAddr {
    let host = uri.rsplit('@').next().unwrap_or_default();
    host.parse()
        .unwrap_or_else(|e| panic!("{host}: not an address and port ({e}); check the peer profile"))
}

pub(crate) fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("valid")
}

/// A recognisable clip: a 440 Hz tone with an envelope, so a test that recorded silence could
/// not pass for one that heard the call.
pub(crate) fn tone(milliseconds: usize) -> Vec<i16> {
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

/// The longest run of bytes the two streams share, once aligned.
///
/// `M-3` compares the whole array, because there both ends are sipx and nothing is lost. Here
/// the far end is a foreign RTP stack that starts echoing before the last packet has been sent,
/// so the comparison is over the aligned overlap rather than the whole clip. Within that overlap
/// the claim is unchanged and it is the strong one: byte for byte, because µ-law in and µ-law
/// out means nothing on the path was entitled to transcode it.
pub(crate) fn longest_aligned_match(sent: &[u8], received: &[u8]) -> usize {
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
pub(crate) struct Echoed {
    /// The payload type every packet carried.
    pub payload_types: Vec<u8>,
    /// The payloads, concatenated.
    pub payload: Vec<u8>,
}

/// Play a tone into a connected call and collect what the far end sends back.
///
/// Deliberately in relay mode. The assertion this feeds is about the bytes an independent
/// implementation put on the wire, and a decode step on the way in would be sipx's opinion of
/// those bytes rather than the bytes.
///
/// Relay is *after* the SRTP transform, not instead of it: what `recv_encoded` yields on an
/// encrypted session is the payload as it was once authenticated and decrypted, so the same
/// comparison measures the same thing under either keying.
pub(crate) async fn echo_round_trip(call: &Call) -> (Vec<u8>, Echoed) {
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
        //
        // Two different questions, and `X-40`'s sweep found them sharing one window here. How long
        // the far end takes to *start* echoing is a bound on failure — two jitter buffers filling,
        // a container starting a channel, a loaded runner — while how long a gap means the echo has
        // *ended* is a property of the stream. A single 600 ms window answered both, so a first
        // packet that arrived late left `payload` empty and `assert_echo` reported "no audio came
        // back" on a call that carried it. That is `MediaSession::record_at_least`'s lesson
        // (`X-28`) in the interop harness: widening the one window would only have moved the cliff,
        // so the start gets its own generous deadline and the gap keeps the tight one.
        let mut window = Duration::from_secs(10);
        while let Ok(Some(packet)) = tokio::time::timeout(window, media.recv_encoded()).await {
            echoed.payload_types.push(packet.payload_type);
            echoed.payload.extend_from_slice(&packet.payload);
            window = Duration::from_millis(600);
        }
        echoed
    })
    .1;

    media.set_relay(false);
    (g711::ulaw_encode_all(&source), collected)
}

/// The assertions every direction and every keying share: audio arrived, it arrived on the
/// payload type the negotiation chose, and it is the audio that was sent.
pub(crate) fn assert_echo(sent: &[u8], echoed: &Echoed, expected_payload_type: u8) {
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
