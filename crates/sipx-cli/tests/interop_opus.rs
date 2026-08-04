//! Opus audio against an independently implemented SIP and media peer.
//!
//! The ordinary interop call proof uses G.711 in relay mode so it can compare wire bytes. Opus is
//! lossy and stateful, so its proof has to ask a different question: did a recognisable 48 kHz
//! signal survive decoding and re-encoding at the peer, then decoding here? Both offer/answer
//! roles run that proof with Opus as the only permitted audio codec.

#![cfg(feature = "opus")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss
)]

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use bytes::Bytes;
use sipx_call::{CodecPreference, Codecs, DialOptions, answer_with, dial};
use sipx_media::{Codec, MediaSession};
use sipx_sip::{Method, Uri};
use sipx_transport::{Config as TransportConfig, Target, bind};

const CLOCK_RATE: u32 = 48_000;
const FRAME: usize = 960;
const CLIP_FRAMES: usize = 40;
const CALL_BOUND: Duration = Duration::from_secs(20);
const AUDIO_BOUND: Duration = Duration::from_secs(10);

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("valid loopback")
}

fn echo_uri() -> String {
    std::env::var("SIPX_INTEROP_OPUS_URI").unwrap_or_else(|_| "sip:echo@127.0.0.1:5060".to_owned())
}

fn addr_in(uri: &str) -> SocketAddr {
    uri.rsplit('@')
        .next()
        .expect("a host follows @")
        .parse()
        .expect("the peer URI ends in an address and port")
}

fn answering_port() -> u16 {
    std::env::var("SIPX_INTEROP_OPUS_UA_PORT")
        .ok()
        .and_then(|port| port.parse().ok())
        .unwrap_or(5082)
}

fn opus_only() -> Codecs {
    Codecs::ordered(&[CodecPreference::Opus]).expect("this test binary has Opus")
}

fn tone(samples: usize, hz: f64) -> Vec<i16> {
    (0..samples)
        .map(|i| {
            let time = i as f64 / f64::from(CLOCK_RATE);
            let envelope = (time * 8.0).min(1.0);
            ((time * hz * std::f64::consts::TAU).sin() * 12_000.0 * envelope) as i16
        })
        .collect()
}

fn correlation(one: &[i16], two: &[i16]) -> f64 {
    let count = one.len().min(two.len());
    if count == 0 {
        return 0.0;
    }
    let (mut dot, mut one_squared, mut two_squared) = (0.0, 0.0, 0.0);
    for index in 0..count {
        let one = f64::from(one[index]);
        let two = f64::from(two[index]);
        dot += one * two;
        one_squared += one * one;
        two_squared += two * two;
    }
    if one_squared == 0.0 || two_squared == 0.0 {
        return 0.0;
    }
    dot / (one_squared.sqrt() * two_squared.sqrt())
}

fn best_correlation(source: &[i16], recovered: &[i16]) -> f64 {
    (0..FRAME * 12)
        .filter_map(|lag| Some(correlation(source, recovered.get(lag..)?)))
        .fold(f64::MIN, f64::max)
}

fn assert_negotiated_opus(media: &MediaSession) {
    assert_eq!(
        media.codec(),
        Codec::Opus,
        "the call must not fall back to G.711"
    );
    assert!(
        (96..=127).contains(&media.wire_payload_type()),
        "Opus must use a negotiated dynamic payload type, got {}",
        media.wire_payload_type()
    );
    assert_eq!(
        media.clock_rate(),
        CLOCK_RATE,
        "the Opus RTP timeline must run at 48 kHz"
    );
}

async fn assert_opus_echo(media: &MediaSession, hz: f64) {
    assert_negotiated_opus(media);
    let source = tone(FRAME * CLIP_FRAMES, hz);
    let (_played, recovered) = tokio::join!(
        media.play(&source, FRAME),
        media.record_at_least(source.len(), AUDIO_BOUND),
    );

    assert!(
        recovered.len() >= FRAME * 10,
        "the peer returned only {} decoded samples",
        recovered.len()
    );
    let mean_magnitude = recovered
        .iter()
        .map(|sample| u64::from(sample.unsigned_abs()))
        .sum::<u64>() as f64
        / recovered.len() as f64;
    assert!(
        mean_magnitude > 500.0,
        "the peer returned silence (mean magnitude {mean_magnitude:.1})"
    );

    // Opus is lossy and has algorithmic delay. Correlate after allowing enough lag for both
    // codec passes and the peer's jitter buffer; then prove the comparison rejects another tone.
    let skip = FRAME * 4;
    let recovered = &recovered[skip.min(recovered.len())..];
    let matching = best_correlation(&source[skip..], recovered);
    let unrelated = tone(source.len() - skip, hz + 587.0);
    let wrong = best_correlation(&unrelated, recovered);
    assert!(
        matching > 0.55,
        "the {hz:.0} Hz signal did not survive the foreign codec path: {matching:.3}"
    );
    assert!(
        wrong < matching - 0.15,
        "the signal identity check is not selective: {matching:.3} vs {wrong:.3}"
    );
}

async fn peer_originates() -> std::process::Output {
    let container = std::env::var("SIPX_INTEROP_CONTAINER").unwrap_or_else(|_| {
        panic!("SIPX_INTEROP_CONTAINER is unset; run through tests/interop/run.sh")
    });
    let channel = std::env::var("SIPX_INTEROP_OPUS_ORIGINATE")
        .unwrap_or_else(|_| "PJSIP/sipx-opus-ua".to_owned());
    let mut command = tokio::process::Command::new("docker");
    command.kill_on_drop(true).args([
        "exec",
        &container,
        "asterisk",
        "-rx",
        &format!("channel originate {channel} application Echo"),
    ]);
    tokio::time::timeout(Duration::from_secs(10), command.output())
        .await
        .expect("the bounded originate command completes")
        .expect("the container command runs")
}

#[tokio::test]
#[ignore = "needs an independent Opus peer; see tests/interop/README.md"]
async fn opus_audio_peer_answers_sipx_offer_and_echoes_real_audio() {
    let mut config = TransportConfig::new("127.0.0.1:0".parse().expect("valid"));
    config.sent_by = loopback().to_string();
    let (handle, _incoming) = bind(config).await.expect("binds");

    let uri = echo_uri();
    let to = Uri::parse(Bytes::from(uri.clone())).expect("a SIP URI");
    let options = DialOptions::new("<sip:sipx-opus@127.0.0.1>", loopback())
        .with_codecs(opus_only())
        .with_timeout(Duration::from_secs(15));
    let mut call = tokio::time::timeout(
        CALL_BOUND,
        dial(&handle, Target::udp(addr_in(&uri)), &to, &options),
    )
    .await
    .expect("the peer answers within the call bound")
    .expect("the peer accepts an Opus-only offer");

    assert_opus_echo(call.media(), 523.25).await;
    call.hang_up().await.expect("the peer accepts BYE");
}

#[tokio::test]
#[ignore = "needs an independent Opus peer; see tests/interop/README.md"]
async fn opus_audio_peer_offers_and_sipx_answers_with_real_audio() {
    let local: SocketAddr = format!("127.0.0.1:{}", answering_port())
        .parse()
        .expect("valid");
    let mut config = TransportConfig::new(local);
    config.sent_by = loopback().to_string();
    let (handle, mut incoming) = bind(config)
        .await
        .unwrap_or_else(|error| panic!("binding {local}: {error}"));

    // A JoinSet aborts its tasks on drop. Combined with `kill_on_drop` in `peer_originates`, a
    // failed assertion cannot detach the container command and leave background work behind.
    let mut originations = tokio::task::JoinSet::new();
    originations.spawn(peer_originates());
    let request = tokio::time::timeout(CALL_BOUND, incoming.recv())
        .await
        .expect("the peer offers within the call bound")
        .expect("an incoming request");
    assert_eq!(request.request.method, Method::Invite);
    let mut call = answer_with(&handle, &request, loopback(), opus_only())
        .await
        .expect("sipx answers the independent Opus offer");

    assert_opus_echo(call.media(), 783.99).await;
    call.hang_up().await.expect("the peer accepts BYE");

    let output = originations
        .join_next()
        .await
        .expect("one originate task exists")
        .expect("the originate task completes");
    assert!(
        output.status.success(),
        "the peer refused to place the Opus call: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
