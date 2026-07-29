//! A call carried in Opus, over real sockets.
//!
//! What separates this from the codec's own round-trip test in `sipx-audio` is everything
//! between: the negotiated payload type, the 48 kHz RTP clock, packets of varying size, and an
//! encoder and decoder that carry state across frames. Any of those can be wrong while the
//! codec itself is perfect.

#![cfg(feature = "opus")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss
)]

use std::time::Duration;

use sipx_media::{Codec, Config, MediaPort, MediaSession};

/// 20 ms at Opus's 48 kHz.
const FRAME: usize = 960;

/// How long this test waits for the clip it played to arrive before calling it lost (`X-28`).
/// A bound on failure rather than a window to measure in — see [`MediaSession::record_at_least`].
const DELIVERY_BOUND: Duration = Duration::from_secs(10);

fn tone(samples: usize, hz: f64) -> Vec<i16> {
    (0..samples)
        .map(|i| {
            let t = i as f64 / 48_000.0;
            ((t * hz * std::f64::consts::TAU).sin() * 12_000.0) as i16
        })
        .collect()
}

fn correlation(one: &[i16], two: &[i16]) -> f64 {
    let n = one.len().min(two.len());
    if n == 0 {
        return 0.0;
    }
    let (mut dot, mut a2, mut b2) = (0.0f64, 0.0f64, 0.0f64);
    for i in 0..n {
        let (a, b) = (f64::from(one[i]), f64::from(two[i]));
        dot += a * b;
        a2 += a * a;
        b2 += b * b;
    }
    if a2 == 0.0 || b2 == 0.0 {
        return 0.0;
    }
    dot / (a2.sqrt() * b2.sqrt())
}

/// Best correlation over a range of lags: Opus has an algorithmic delay, and the network adds
/// its own. Measuring at lag zero would be measuring the delay, not the audio.
fn best_correlation(source: &[i16], recovered: &[i16]) -> f64 {
    (0..FRAME * 4)
        .filter_map(|lag| Some(correlation(source, recovered.get(lag..)?)))
        .fold(f64::MIN, f64::max)
}

/// Two sessions speaking Opus on a payload type SDP could plausibly have negotiated.
async fn opus_pair(payload_type: u8) -> (MediaSession, MediaSession) {
    let one = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let two = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let (one_addr, two_addr) = (one.local_addr(), two.local_addr());

    let configure = |remote| {
        let mut config = Config::new(remote, Codec::Opus);
        config.payload_type = Some(payload_type);
        config.rtcp_interval = None;
        config
    };
    (
        one.start(configure(two_addr)),
        two.start(configure(one_addr)),
    )
}

/// M-13's exit criterion.
#[tokio::test]
async fn an_opus_call_carries_audio_that_survives_the_round_trip() {
    let (alice, bob) = opus_pair(111).await;

    let source = tone(FRAME * 40, 440.0);
    let (_played, heard) = tokio::join!(
        alice.play(&source, FRAME),
        bob.record_at_least(source.len(), DELIVERY_BOUND),
    );

    assert!(
        heard.len() > FRAME * 10,
        "the call carried almost nothing: {} samples",
        heard.len()
    );

    // Skip the encoder settling, then compare waveforms. Opus is lossy, so sample equality is
    // not the question — whether it is still the same tone is.
    let skip = FRAME * 4;
    let correlation = best_correlation(&source[skip..], &heard[skip.min(heard.len())..]);
    assert!(
        correlation > 0.85,
        "the tone did not survive the call: correlation {correlation:.3}"
    );

    // And it is that tone, not any tone: an unrelated frequency must correlate much worse.
    let unrelated = tone(source.len() - skip, 1_500.0);
    let against_wrong = best_correlation(&unrelated, &heard[skip.min(heard.len())..]);
    assert!(
        against_wrong < correlation - 0.2,
        "the comparison would match anything: {against_wrong:.3} vs {correlation:.3}"
    );

    alice.stop();
    bob.stop();
}

/// The negotiated number, not sipx's preferred one. Two endpoints routinely pick different
/// numbers for the same codec, and a session that sent its own would be sending Opus on a
/// payload type the far end has assigned to something else.
#[tokio::test]
async fn opus_travels_on_the_negotiated_payload_type() {
    use bytes::Bytes;
    use tokio::net::UdpSocket;

    let peer = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let peer_addr = peer.local_addr().expect("has an address");
    let port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");

    let mut config = Config::new(peer_addr, Codec::Opus);
    // 96, not the 111 sipx would have proposed.
    config.payload_type = Some(96);
    config.rtcp_interval = None;
    let session = port.start(config);

    let playing = tokio::spawn(async move {
        session.play(&tone(FRAME * 10, 440.0), FRAME).await;
        session
    });

    let mut datagram = vec![0u8; 2048];
    let (len, _) = tokio::time::timeout(Duration::from_secs(5), peer.recv_from(&mut datagram))
        .await
        .expect("no timeout")
        .expect("receives");
    let packet = sipx_rtp::Packet::decode(&Bytes::copy_from_slice(&datagram[..len]))
        .expect("a valid RTP packet");

    assert_eq!(
        packet.payload_type, 96,
        "the wire must carry the negotiated number"
    );
    assert!(
        !packet.payload.is_empty() && packet.payload.len() < FRAME * 2,
        "an Opus payload is compressed, not PCM: {} bytes",
        packet.payload.len()
    );

    playing.await.expect("finishes").stop();
}

/// The RTP clock is 48000 for Opus whatever the audio rate (RFC 7587 §7), and the timestamp
/// must advance by the samples the packet carried. A session using 8000 would build a timeline
/// six times too slow and the far end would play the call with gaps between every packet.
#[tokio::test]
async fn the_rtp_timestamp_advances_at_the_opus_clock() {
    use bytes::Bytes;
    use tokio::net::UdpSocket;

    let peer = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let peer_addr = peer.local_addr().expect("has an address");
    let port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let mut config = Config::new(peer_addr, Codec::Opus);
    config.payload_type = Some(111);
    config.rtcp_interval = None;
    assert_eq!(config.clock_rate, 48_000, "RFC 7587 §7");
    let session = port.start(config);

    let playing = tokio::spawn(async move {
        session.play(&tone(FRAME * 10, 440.0), FRAME).await;
        session
    });

    let mut datagram = vec![0u8; 2048];
    let mut stamps = Vec::new();
    for _ in 0..3 {
        let (len, _) = tokio::time::timeout(Duration::from_secs(5), peer.recv_from(&mut datagram))
            .await
            .expect("no timeout")
            .expect("receives");
        let packet = sipx_rtp::Packet::decode(&Bytes::copy_from_slice(&datagram[..len]))
            .expect("a valid RTP packet");
        stamps.push(packet.timestamp);
    }

    let step = stamps[1].wrapping_sub(stamps[0]);
    assert_eq!(
        step, FRAME as u32,
        "20 ms at 48 kHz is 960 timestamp units, not {step}"
    );
    assert_eq!(stamps[2].wrapping_sub(stamps[1]), FRAME as u32);

    playing.await.expect("finishes").stop();
}
