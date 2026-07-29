//! A conference, with real sockets for every participant.
//!
//! Three parties, because two is a bridge. The property that separates a conference from a
//! broadcast — each participant hearing everyone *but themselves* — cannot be observed with
//! fewer than three: with two, "everyone else" and "the other one" are the same set.

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

use sipx_media::{Codec, Conference, Config, MediaPort, MediaSession};

/// A tone at a distinguishable frequency, so a mix can be told apart from a single voice.
fn tone(hz: f64, milliseconds: usize, amplitude: f64) -> Vec<i16> {
    (0..milliseconds * 8)
        .map(|i| {
            let t = i as f64 / 8000.0;
            ((t * hz * std::f64::consts::TAU).sin() * amplitude) as i16
        })
        .collect()
}

/// A participant: a session in the conference, and the far end that talks and listens.
struct Party {
    far: MediaSession,
    near: Arc<MediaSession>,
}

async fn party() -> Party {
    let far_port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let near_port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let (far_addr, near_addr) = (far_port.local_addr(), near_port.local_addr());

    let mut far_config = Config::new(near_addr, Codec::Pcmu);
    far_config.rtcp_interval = None;
    let mut near_config = Config::new(far_addr, Codec::Pcmu);
    near_config.rtcp_interval = None;

    Party {
        far: far_port.start(far_config),
        near: Arc::new(near_port.start(near_config)),
    }
}

/// How long a test here waits for a mixed stream to deliver before calling it lost (`X-28`).
/// A bound on failure, not a window to measure in — see [`MediaSession::record_at_least`].
const DELIVERY_BOUND: Duration = Duration::from_secs(10);

/// Record a fixed number of samples of the mixed stream.
///
/// Not `record_until_idle`: a conference sends *continuously*. Every participant gets a frame
/// every 20 ms whether anyone is talking or not, because that is what a mixed stream is — so
/// the silence a conference produces still arrives as packets, and waiting for a gap waits
/// forever.
///
/// Counted rather than timed, which is what makes that continuity useful (`X-28`). This used to
/// record for a fixed 600 ms of wall clock, and a fixed window against real sockets measures
/// how fast the machine is: under load it returned a handful of frames or none, `peak` fell to
/// zero, and the test reported that a participant could not hear the conference. Because the
/// mixer never stops sending, *every* participant reaches any count asked of it — the ones
/// asserted to hear silence included — so counting terminates here for the same reason waiting
/// for a gap does not.
async fn record_mixed(session: &MediaSession, samples: usize) -> Vec<i16> {
    session.record_at_least(samples, DELIVERY_BOUND).await
}

/// The peak absolute sample, which is how loud something is.
fn peak(samples: &[i16]) -> i32 {
    samples
        .iter()
        .map(|s| i32::from(s.abs()))
        .max()
        .unwrap_or(0)
}

/// M-12's exit criterion.
///
/// Alice talks loudly and nobody else says anything. Bob and Carol must hear her; Alice must
/// hear silence. Hearing herself would be hearing her own voice a round trip late, which is the
/// single most disorienting artefact in conferencing.
#[tokio::test]
async fn no_participant_hears_their_own_audio() {
    let alice = party().await;
    let bob = party().await;
    let carol = party().await;

    let conference = Conference::narrowband();
    conference.join(Arc::clone(&alice.near)).await;
    conference.join(Arc::clone(&bob.near)).await;
    conference.join(Arc::clone(&carol.near)).await;
    assert_eq!(conference.len().await, 3);

    let voice = tone(440.0, 500, 10_000.0);
    let (_played, heard_by_alice, heard_by_bob, heard_by_carol) = tokio::join!(
        alice.far.play(&voice, 160),
        record_mixed(&alice.far, voice.len()),
        record_mixed(&bob.far, voice.len()),
        record_mixed(&carol.far, voice.len()),
    );

    assert!(
        peak(&heard_by_bob) > 3000,
        "Bob must hear Alice: peak {}",
        peak(&heard_by_bob)
    );
    assert!(
        peak(&heard_by_carol) > 3000,
        "Carol must hear Alice: peak {}",
        peak(&heard_by_carol)
    );
    assert!(
        peak(&heard_by_alice) < 1000,
        "Alice must not hear herself: peak {} over {} samples",
        peak(&heard_by_alice),
        heard_by_alice.len()
    );

    conference.close().await;
}

/// Two people talking at once. Each of the others hears both, which is the thing a conference
/// does that a pair of bridges does not.
#[tokio::test]
async fn a_participant_hears_everyone_else_at_once() {
    let alice = party().await;
    let bob = party().await;
    let carol = party().await;

    let conference = Conference::narrowband();
    conference.join(Arc::clone(&alice.near)).await;
    conference.join(Arc::clone(&bob.near)).await;
    conference.join(Arc::clone(&carol.near)).await;

    // Alice and Bob talk at once, each at a third of full scale, so the sum is loud but does
    // not clip — a clipped mix would pass a loudness check for the wrong reason.
    let voice = tone(440.0, 500, 8_000.0);
    let other = tone(880.0, 500, 8_000.0);
    let (_played, _played_other, heard_by_carol) = tokio::join!(
        alice.far.play(&voice, 160),
        bob.far.play(&other, 160),
        record_mixed(&carol.far, voice.len()),
    );

    // The sum of two 8000-amplitude tones at different frequencies exceeds either alone.
    assert!(
        peak(&heard_by_carol) > 9000,
        "Carol should hear both voices summed, not one: peak {}",
        peak(&heard_by_carol)
    );

    conference.close().await;
}

/// Joining and leaving must not disturb anyone else. The conference clock does not stop, and
/// the people already talking go on hearing each other.
#[tokio::test]
async fn participants_join_and_leave_without_disturbing_the_others() {
    let alice = party().await;
    let bob = party().await;
    let carol = party().await;

    let conference = Conference::narrowband();
    conference.join(Arc::clone(&alice.near)).await;
    let bob_id = conference.join(Arc::clone(&bob.near)).await;

    // Carol joins while Alice is talking.
    let voice = tone(440.0, 600, 10_000.0);
    let joining = {
        let carol_near = Arc::clone(&carol.near);
        async {
            tokio::time::sleep(Duration::from_millis(150)).await;
            conference.join(carol_near).await
        }
    };
    let (_played, heard_by_bob, heard_by_carol, _carol_id) = tokio::join!(
        alice.far.play(&voice, 160),
        record_mixed(&bob.far, voice.len()),
        record_mixed(&carol.far, voice.len()),
        joining,
    );

    assert!(
        peak(&heard_by_bob) > 3000,
        "Bob was already listening and must not have been interrupted: peak {}",
        peak(&heard_by_bob)
    );
    assert!(
        peak(&heard_by_carol) > 3000,
        "Carol joined mid-call and must hear what followed: peak {}",
        peak(&heard_by_carol)
    );
    assert_eq!(conference.len().await, 3);

    // Bob leaves; Alice and Carol carry on.
    conference.leave(bob_id).await;
    assert_eq!(conference.len().await, 2);

    let (_played, heard_after) = tokio::join!(
        alice.far.play(&voice, 160),
        record_mixed(&carol.far, voice.len()),
    );
    assert!(
        peak(&heard_after) > 3000,
        "the conference must survive somebody leaving: peak {}",
        peak(&heard_after)
    );

    conference.close().await;
}

/// A participant who has left stops being heard. Without this, "leave" would be cosmetic.
#[tokio::test]
async fn someone_who_has_left_is_no_longer_mixed_in() {
    let alice = party().await;
    let bob = party().await;

    let conference = Conference::narrowband();
    let alice_id = conference.join(Arc::clone(&alice.near)).await;
    conference.join(Arc::clone(&bob.near)).await;

    conference.leave(alice_id).await;

    let voice = tone(440.0, 400, 12_000.0);
    let (_played, heard_by_bob) = tokio::join!(
        alice.far.play(&voice, 160),
        record_mixed(&bob.far, voice.len()),
    );
    assert!(
        peak(&heard_by_bob) < 1000,
        "somebody who left must not still be audible: peak {}",
        peak(&heard_by_bob)
    );

    conference.close().await;
}

/// One person alone hears silence, not themselves.
#[tokio::test]
async fn a_lone_participant_hears_silence() {
    let alice = party().await;
    let conference = Conference::narrowband();
    conference.join(Arc::clone(&alice.near)).await;

    let voice = tone(440.0, 400, 12_000.0);
    let (_played, heard) = tokio::join!(
        alice.far.play(&voice, 160),
        record_mixed(&alice.far, voice.len()),
    );
    assert!(
        peak(&heard) < 1000,
        "a conference of one is silence, not an echo: peak {}",
        peak(&heard)
    );

    conference.close().await;
}
