//! Encrypted media, over real sockets.
//!
//! The unit tests in `sipx-rtp` prove the transform matches RFC 3711's published vectors. These
//! prove the session actually uses it — that what leaves the socket is unreadable, and that a
//! session expecting SRTP cannot be made to accept anything else.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss
)]

use std::time::Duration;

use bytes::Bytes;
use sipx_media::{Codec, Config, MediaPort, MediaSession, SrtpKeys};
use sipx_rtp::srtp::Profile;
use tokio::net::UdpSocket;

/// Every profile a call can negotiate. The socket-level tests below run over all of them rather
/// than over the counter-mode one they were written for: what leaves the socket has to be
/// unreadable under each transform, not under the one that happened to ship first (`M-41`).
const EVERY_PROFILE: [Profile; 3] = Profile::STRONGEST_FIRST;

/// How long a test here waits for a clip it played to arrive before calling it lost (`X-28`).
/// A bound on failure rather than a window to measure in — see [`MediaSession::record_at_least`].
const DELIVERY_BOUND: Duration = Duration::from_secs(10);

/// A distinctive signal: µ-law encodes a constant amplitude to a constant byte, so a recognisable
/// byte pattern appears verbatim in an *unencrypted* payload and cannot in an encrypted one.
fn recognisable() -> Vec<i16> {
    // 8000 is well inside range and encodes to one repeated µ-law code.
    vec![8000i16; 8000]
}

/// Two matched key sets for one profile, sized from it rather than written out.
fn keys(profile: Profile) -> (SrtpKeys, SrtpKeys) {
    let (key_len, salt_len) = profile.key_and_salt_len();
    let (a_key, a_salt) = (vec![0x11; key_len], vec![0x22; salt_len]);
    let (b_key, b_salt) = (vec![0x33; key_len], vec![0x44; salt_len]);
    (
        SrtpKeys {
            profile,
            local: (a_key.clone(), a_salt.clone()),
            remote: (b_key.clone(), b_salt.clone()),
        },
        SrtpKeys {
            profile,
            local: (b_key, b_salt),
            remote: (a_key, a_salt),
        },
    )
}

/// Two sessions on loopback, keyed for `srtp`'s profile or unencrypted.
async fn pair_keyed(srtp: Option<Profile>) -> (MediaSession, MediaSession) {
    let one = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let two = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let (one_addr, two_addr) = (one.local_addr(), two.local_addr());

    let mut config_one = Config::new(two_addr, Codec::Pcmu);
    config_one.rtcp_interval = None;
    let mut config_two = Config::new(one_addr, Codec::Pcmu);
    config_two.rtcp_interval = None;
    if let Some(profile) = srtp {
        let (one_keys, two_keys) = keys(profile);
        config_one.srtp = Some(one_keys);
        config_two.srtp = Some(two_keys);
    }

    (
        one.start(config_one).expect("valid media setup"),
        two.start(config_two).expect("valid media setup"),
    )
}

/// M-14's exit criterion, under every profile. What leaves the socket must not be the audio that
/// went in, and the packet must be exactly its header, its ciphertext and **that profile's** tag.
#[tokio::test]
async fn media_on_a_secure_call_is_not_readable_from_the_wire() {
    for profile in EVERY_PROFILE {
        // A tap standing where anyone on the path stands.
        let tap = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
        let tap_addr = tap.local_addr().expect("has an address");

        let port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
            .await
            .expect("binds");
        let (local_keys, _) = keys(profile);
        let mut config = Config::new(tap_addr, Codec::Pcmu);
        config.rtcp_interval = None;
        config.srtp = Some(local_keys);
        let session = port.start(config).expect("valid media setup");

        let audio = recognisable();
        let expected_code = sipx_audio::g711::ulaw_encode(audio[0]);
        let playing = tokio::spawn(async move {
            session.play(&audio, 160).await;
            session
        });

        let mut datagram = vec![0u8; 2048];
        let (len, _) = tokio::time::timeout(Duration::from_secs(5), tap.recv_from(&mut datagram))
            .await
            .expect("no timeout")
            .expect("a packet arrives");
        let seen = &datagram[..len];

        // The header is readable — a relay needs it, and both RFC 3711 and RFC 7714 §8.2 leave it
        // in the clear, the latter as Associated Data.
        assert_eq!(seen[0] >> 6, 2, "{profile:?}: still an RTP packet");
        assert_eq!(
            seen[1] & 0x7F,
            Codec::Pcmu.payload_type(),
            "{profile:?}: payload type is visible"
        );

        // The payload is not. Unencrypted, every octet after the 12-byte header would be the same
        // µ-law code; encrypted, a run of twenty of them cannot survive.
        let payload = &seen[12..len - profile.tag_len()];
        assert!(
            !payload
                .windows(20)
                .any(|w| w.iter().all(|b| *b == expected_code)),
            "{profile:?}: the audio is readable on the wire — a run of the plaintext µ-law code"
        );
        // And the tag is there, which is what makes the header unforgeable. Its length is the
        // profile's: 80 bits for counter mode, and RFC 7714 §13.2's 128 for the AEAD profiles.
        assert_eq!(
            len,
            12 + 160 + profile.tag_len(),
            "{profile:?}: header, encrypted payload, and this profile's tag"
        );

        playing.await.expect("finishes").stop();
    }
}

/// The other half: encrypted media still *is* the audio at the far end, under every profile.
#[tokio::test]
async fn an_encrypted_call_still_carries_the_audio() {
    for profile in EVERY_PROFILE {
        let (alice, bob) = pair_keyed(Some(profile)).await;

        let audio = recognisable();
        let (_played, heard) = tokio::join!(
            alice.play(&audio, 160),
            bob.record_at_least(audio.len(), DELIVERY_BOUND),
        );

        assert!(
            heard.len() > audio.len() / 2,
            "{profile:?}: most of the audio should arrive: {} of {}",
            heard.len(),
            audio.len()
        );
        // µ-law round-trips exactly for a value already on its grid, so this is bit-for-bit.
        let expected = sipx_audio::g711::ulaw_decode(sipx_audio::g711::ulaw_encode(8000));
        assert!(
            heard.iter().all(|s| *s == expected),
            "{profile:?}: the decrypted audio is not what was sent"
        );

        alice.stop();
        bob.stop();
    }
}

/// A session expecting SRTP must not accept plain RTP. Otherwise an attacker downgrades the call
/// by sending one unencrypted packet, and the encryption becomes decorative.
#[tokio::test]
async fn a_session_expecting_srtp_refuses_plain_rtp() {
    for profile in EVERY_PROFILE {
        let peer = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
        let peer_addr = peer.local_addr().expect("has an address");
        let port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
            .await
            .expect("binds");
        let session_addr = port.local_addr();

        let (local_keys, _) = keys(profile);
        let mut config = Config::new(peer_addr, Codec::Pcmu);
        config.rtcp_interval = None;
        config.srtp = Some(local_keys);
        let session = port.start(config).expect("valid media setup");

        // A perfectly well-formed *plain* RTP packet.
        let plain = sipx_rtp::Packet::new(
            Codec::Pcmu.payload_type(),
            1,
            160,
            0xABCD_1234,
            Bytes::from(vec![0xFFu8; 160]),
        )
        .encode();
        peer.send_to(&plain, session_addr).await.expect("sends");

        let heard = tokio::time::timeout(Duration::from_millis(400), session.recv()).await;
        assert!(
            heard.is_err(),
            "{profile:?}: a session expecting SRTP accepted unencrypted media"
        );

        session.stop();
    }
}

/// And a packet encrypted under the wrong key is refused rather than played as noise. A decoder
/// pushed past a failed authentication produces a burst that is louder than silence.
#[tokio::test]
async fn a_packet_under_the_wrong_key_is_refused() {
    for profile in EVERY_PROFILE {
        let peer = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
        let peer_addr = peer.local_addr().expect("has an address");
        let port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
            .await
            .expect("binds");
        let session_addr = port.local_addr();

        let (local_keys, _) = keys(profile);
        let mut config = Config::new(peer_addr, Codec::Pcmu);
        config.rtcp_interval = None;
        config.srtp = Some(local_keys);
        let session = port.start(config).expect("valid media setup");

        // Encrypted, but by somebody else — and under the same profile, so the refusal is about
        // the key rather than about a packet the session could not parse at all.
        let (key_len, salt_len) = profile.key_and_salt_len();
        let mut stranger =
            sipx_rtp::SrtpContext::new(profile, &vec![0xEE; key_len], &vec![0xDD; salt_len])
                .expect("a context");
        let plain = sipx_rtp::Packet::new(
            Codec::Pcmu.payload_type(),
            1,
            160,
            0xABCD_1234,
            Bytes::from(vec![0xFFu8; 160]),
        )
        .encode();
        let forged = stranger.protect(&plain).expect("protects");
        peer.send_to(&forged, session_addr).await.expect("sends");

        let heard = tokio::time::timeout(Duration::from_millis(400), session.recv()).await;
        assert!(
            heard.is_err(),
            "{profile:?}: a packet under the wrong key was played"
        );

        session.stop();
    }
}

// -----------------------------------------------------------------------------------------------
// Turning an SDES answer into keys (RFC 4568 §5.1.3, `docs/specs/srtp.md` §5.4).
// -----------------------------------------------------------------------------------------------

fn offered(tag: u32) -> sipx_sdp::crypto::Crypto {
    offered_as(tag, sipx_sdp::crypto::Suite::AesCm128HmacSha1_80)
}

fn offered_as(tag: u32, suite: sipx_sdp::crypto::Suite) -> sipx_sdp::crypto::Crypto {
    sipx_sdp::crypto::Crypto::offer(tag, suite, true).expect("secure signalling")
}

/// The keys an answer settles on carry the **negotiated** transform, not a default.
///
/// This is `M-41`'s end-to-end claim at the seam the profile used to be discarded at: before,
/// `SrtpKeys` held two byte pairs and the session inferred a cipher; now the suite that was agreed
/// decides which one is installed, for every suite. Getting it wrong here is not a crash — it is
/// a stream protected by a transform the far end did not agree to.
#[test]
fn the_keys_an_answer_settles_on_carry_the_negotiated_profile() {
    for (suite, expected) in [
        (
            sipx_sdp::crypto::Suite::AesCm128HmacSha1_80,
            Profile::AesCm128HmacSha1_80,
        ),
        (
            sipx_sdp::crypto::Suite::AeadAes128Gcm,
            Profile::AeadAes128Gcm,
        ),
        (
            sipx_sdp::crypto::Suite::AeadAes256Gcm,
            Profile::AeadAes256Gcm,
        ),
    ] {
        let ours = offered_as(2, suite);
        let theirs = offered_as(2, suite);
        let keys = SrtpKeys::from_answer(std::slice::from_ref(&ours), Some(&theirs))
            .expect("the tag and suite were offered");
        assert_eq!(keys.profile, expected, "{suite:?}");
        // And the key material is the length that profile requires, so the context it is handed
        // to cannot refuse it.
        let (key_len, salt_len) = expected.key_and_salt_len();
        assert_eq!(
            (keys.local.0.len(), keys.local.1.len()),
            (key_len, salt_len)
        );
        assert_eq!(
            (keys.remote.0.len(), keys.remote.1.len()),
            (key_len, salt_len)
        );
    }
}

/// An answer that echoes the tag but renames the transform is refused (RFC 4568 §5.1.3).
///
/// The check is on tag **and** suite together. Matching on the tag alone would accept an answer
/// that kept a number this side recognised and swapped the cipher under it — which is exactly the
/// substitution `docs/designs/media-runtime-safety.md` forbids, and it becomes reachable the
/// moment an offer carries more than one suite.
#[test]
fn an_answer_that_keeps_the_tag_and_changes_the_suite_is_refused() {
    let ours = offered_as(1, sipx_sdp::crypto::Suite::AeadAes256Gcm);
    let theirs = offered_as(1, sipx_sdp::crypto::Suite::AeadAes128Gcm);
    assert!(
        SrtpKeys::from_answer(std::slice::from_ref(&ours), Some(&theirs)).is_err(),
        "tag 1 named AEAD_AES_256_GCM in the offer and a different transform in the answer"
    );
}

/// An answer that echoed the tag keys the session: our half from the offer we sent, their half
/// from the answer.
#[test]
fn an_answer_that_echoes_the_offered_tag_produces_keys() {
    let ours = offered(3);
    let theirs = offered(3);
    let keys = SrtpKeys::from_answer(std::slice::from_ref(&ours), Some(&theirs))
        .expect("the tag was offered");

    assert_eq!(
        keys.local.0,
        ours.master_key(),
        "we protect with our own key"
    );
    assert_eq!(
        keys.remote.0,
        theirs.master_key(),
        "and unprotect with theirs"
    );
}

/// And one that did not is refused **as an error**, not as an unkeyed session. A media path that
/// quietly degrades to no encryption because a tag mismatched is worse than a call that fails:
/// nothing tells the application, and the user hears an unprotected call as a protected one.
#[test]
fn an_answer_whose_tag_was_never_offered_produces_no_keys() {
    let ours = offered(3);
    let theirs = offered(8);
    let refused = SrtpKeys::from_answer(std::slice::from_ref(&ours), Some(&theirs));
    assert!(
        refused.is_err(),
        "keys were built from an answer nobody agreed on"
    );
}

/// An `RTP/SAVP` answer that carried no `a=crypto` at all is the same failure. §5.1.3 requires
/// the offerer to verify that the answer contains a key.
#[test]
fn an_answer_with_no_crypto_at_all_produces_no_keys() {
    let ours = offered(1);
    assert!(SrtpKeys::from_answer(std::slice::from_ref(&ours), None).is_err());
}

/// Unencrypted calls are untouched by any of this.
#[tokio::test]
async fn a_plain_call_still_works() {
    let (alice, bob) = pair_keyed(None).await;
    let audio = recognisable();
    let (_played, heard) = tokio::join!(
        alice.play(&audio, 160),
        bob.record_at_least(audio.len(), DELIVERY_BOUND),
    );
    assert!(heard.len() > audio.len() / 2, "{} samples", heard.len());
    alice.stop();
    bob.stop();
}
