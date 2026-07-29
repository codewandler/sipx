//! Two calls connected, with real sockets on both legs.
//!
//! Four sessions rather than two, because a bridge is between *calls*: each leg has its own
//! remote end, and a bridge that mixed the two legs up would still pass this test with two.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss
)]

use std::sync::Arc;
use std::time::Duration;

use sipx_media::{Bridge, Codec, Config, MediaPort, MediaSession};

/// How long a test here will wait for a clip it played to come out the other side before calling
/// it lost (`X-28`).
///
/// A bound on failure, not a window to measure in: every clip below is under half a second, so
/// this is more than twenty times the honest answer and nothing that arrives inside it is late
/// in any sense a test should care about. What it buys is that a machine with four other gates
/// compiling on it produces the same verdict as an idle one — slower, but the same. The old
/// `record_until_idle(400ms)` spent one duration on both "has it started" and "has it finished",
/// and under load neither was a property of the audio; see [`MediaSession::record_at_least`].
const DELIVERY_BOUND: Duration = Duration::from_secs(10);

/// A recognisable clip, so a test that recorded silence could not pass.
fn tone(milliseconds: usize) -> Vec<i16> {
    (0..milliseconds * 8)
        .map(|i| {
            let t = i as f64 / 8000.0;
            ((t * 440.0 * std::f64::consts::TAU).sin() * 12000.0) as i16
        })
        .collect()
}

/// Two sessions pointed at each other on loopback.
async fn pair(codec_one: Codec, codec_two: Codec) -> (MediaSession, MediaSession) {
    let one = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let two = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let (one_addr, two_addr) = (one.local_addr(), two.local_addr());

    let mut config_one = Config::new(two_addr, codec_one);
    config_one.rtcp_interval = None;
    let mut config_two = Config::new(one_addr, codec_two);
    config_two.rtcp_interval = None;

    (one.start(config_one), two.start(config_two))
}

/// M-11's exit criterion. `alice` and `bob` are the two far ends; `left` and `right` are the two
/// legs of the bridged call in the middle.
#[tokio::test]
async fn audio_played_into_one_call_is_heard_on_the_other() {
    let (alice, left) = pair(Codec::Pcmu, Codec::Pcmu).await;
    let (right, bob) = pair(Codec::Pcmu, Codec::Pcmu).await;

    let bridge = Bridge::connect(Arc::new(left), Arc::new(right));
    assert!(
        !bridge.is_transcoding(),
        "two legs on the same codec must not be transcoded"
    );

    let clip = tone(400);
    let (_played, heard) = tokio::join!(
        alice.play(&clip, 160),
        bob.record_at_least(clip.len(), DELIVERY_BOUND)
    );

    assert!(
        heard.len() > clip.len() / 2,
        "Bob should have heard most of what Alice played: {} of {}",
        heard.len(),
        clip.len()
    );
    let loudest = heard.iter().map(|s| s.abs()).max().unwrap_or(0);
    assert!(
        loudest > 4000,
        "what arrived was silence, not the tone: peak {loudest}"
    );

    bridge.close();
    alice.stop();
    bob.stop();
}

/// And the other direction, on the same bridge. A bridge that forwarded one way only would pass
/// the test above and be a call where one party cannot be heard.
#[tokio::test]
async fn a_bridge_carries_audio_both_ways() {
    let (alice, left) = pair(Codec::Pcmu, Codec::Pcmu).await;
    let (right, bob) = pair(Codec::Pcmu, Codec::Pcmu).await;
    let bridge = Bridge::connect(Arc::new(left), Arc::new(right));

    let clip = tone(300);
    let (_played, heard_by_alice) = tokio::join!(
        bob.play(&clip, 160),
        alice.record_at_least(clip.len(), DELIVERY_BOUND)
    );
    assert!(
        heard_by_alice.len() > clip.len() / 2,
        "Alice should have heard Bob: {} samples",
        heard_by_alice.len()
    );

    bridge.close();
    alice.stop();
    bob.stop();
}

/// Different codecs leave no choice, and the bridge says so.
#[tokio::test]
async fn differing_codecs_are_transcoded_and_the_fact_is_reported() {
    let (alice, left) = pair(Codec::Pcmu, Codec::Pcmu).await;
    let (right, bob) = pair(Codec::Pcma, Codec::Pcma).await;

    let bridge = Bridge::connect(Arc::new(left), Arc::new(right));
    assert!(
        bridge.is_transcoding(),
        "µ-law on one leg and A-law on the other cannot be passed through"
    );

    let clip = tone(400);
    let (_played, heard) = tokio::join!(
        alice.play(&clip, 160),
        bob.record_at_least(clip.len(), DELIVERY_BOUND)
    );
    assert!(
        heard.len() > clip.len() / 2,
        "transcoded audio must still arrive: {} samples",
        heard.len()
    );
    let loudest = heard.iter().map(|s| s.abs()).max().unwrap_or(0);
    assert!(loudest > 4000, "peak {loudest}");

    bridge.close();
    alice.stop();
    bob.stop();
}

/// Pass-through really is pass-through: the samples that come out are bit-for-bit the samples
/// that went in.
///
/// Note what this does *not* prove. G.711 decoding is exactly invertible, so a bridge that
/// decoded and re-encoded would deliver the same bytes and pass this too. The mechanism is
/// asserted directly in the test below; this one asserts the outcome a caller cares about.
#[tokio::test]
async fn a_pass_through_bridge_delivers_the_audio_unchanged() {
    let (alice, left) = pair(Codec::Pcmu, Codec::Pcmu).await;
    let (right, bob) = pair(Codec::Pcmu, Codec::Pcmu).await;
    let bridge = Bridge::connect(Arc::new(left), Arc::new(right));

    // A signal already on the µ-law grid, so the encode at Alice's end is the only quantisation
    // and any difference at Bob's end was introduced in between.
    let clip: Vec<i16> = tone(300)
        .into_iter()
        .map(|s| sipx_audio::g711::ulaw_decode(sipx_audio::g711::ulaw_encode(s)))
        .collect();

    let (_played, heard) = tokio::join!(
        alice.play(&clip, 160),
        bob.record_at_least(clip.len(), DELIVERY_BOUND)
    );

    assert!(heard.len() >= 160, "something must have arrived");
    // Line the two up: the far end starts recording mid-stream, so compare on the overlap.
    let start = clip.len().saturating_sub(heard.len());
    let expected = &clip[start..];
    let common = expected.len().min(heard.len());
    let differing = (0..common).filter(|&i| expected[i] != heard[i]).count();
    assert_eq!(
        differing, 0,
        "the bridge changed {differing} of {common} samples"
    );

    bridge.close();
    alice.stop();
    bob.stop();
}

/// The mechanism, asserted where it can be seen: in relay mode a session hands packets on
/// **still encoded** and the decoded channel stays empty. This is what distinguishes a
/// pass-through bridge from one that decodes and re-encodes — an outcome test cannot, because
/// for G.711 the two produce identical bytes.
#[tokio::test]
async fn a_relaying_session_hands_packets_on_without_decoding_them() {
    let (alice, left) = pair(Codec::Pcmu, Codec::Pcmu).await;
    left.set_relay(true);

    let clip = tone(200);
    let playing = tokio::spawn(async move {
        alice.play(&clip, 160).await;
        alice
    });

    let encoded = tokio::time::timeout(Duration::from_secs(5), left.recv_encoded())
        .await
        .expect("no timeout")
        .expect("a packet arrives still encoded");
    assert_eq!(encoded.payload_type, Codec::Pcmu.payload_type());
    assert_eq!(
        encoded.payload.len(),
        160,
        "one packet's worth of µ-law, not decoded samples"
    );

    // And nothing went down the decoded path.
    let decoded = tokio::time::timeout(Duration::from_millis(200), left.recv()).await;
    assert!(
        decoded.is_err(),
        "a relaying session must not also decode; that would deliver every packet twice"
    );

    let alice = playing.await.expect("finishes");
    alice.stop();
    left.stop();
}

/// A bridge ends when either call does, and takes its tasks with it.
#[tokio::test]
async fn a_bridge_ends_when_either_call_does() {
    let (alice, left) = pair(Codec::Pcmu, Codec::Pcmu).await;
    let (right, bob) = pair(Codec::Pcmu, Codec::Pcmu).await;
    let left = Arc::new(left);
    let bridge = Bridge::connect(Arc::clone(&left), Arc::new(right));
    assert!(bridge.is_connected());

    // One leg hangs up.
    left.stop();

    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        while bridge.is_connected() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(
        ended.is_ok(),
        "a bridge whose leg has stopped must not go on forwarding"
    );

    alice.stop();
    bob.stop();
}

/// Dropping a bridge stops it. Without this the forwarding tasks keep the sessions alive
/// through their handles, so the sockets stay open and nothing is ever reclaimed.
#[tokio::test]
async fn dropping_a_bridge_stops_the_forwarding() {
    let (alice, left) = pair(Codec::Pcmu, Codec::Pcmu).await;
    let (right, bob) = pair(Codec::Pcmu, Codec::Pcmu).await;
    let right = Arc::new(right);

    let bridge = Bridge::connect(Arc::new(left), Arc::clone(&right));
    drop(bridge);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Nothing crosses any more.
    //
    // Still `record_until_idle`, and deliberately (`X-28`). This one asserts the recording is
    // *empty*, so the fixed window is a window to look in rather than a deadline to beat: a
    // loaded machine can only make it emptier, never falsely full. Waiting for a sample count
    // that must never arrive would be a ten-second sleep in every run.
    let clip = tone(200);
    let (_played, heard) = tokio::join!(
        alice.play(&clip, 160),
        bob.record_until_idle(Duration::from_millis(250))
    );
    assert!(
        heard.is_empty(),
        "a dropped bridge went on forwarding {} samples",
        heard.len()
    );

    alice.stop();
    bob.stop();
    right.stop();
}
