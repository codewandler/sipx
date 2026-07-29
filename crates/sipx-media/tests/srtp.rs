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
use tokio::net::UdpSocket;

/// A distinctive signal: µ-law encodes a constant amplitude to a constant byte, so a recognisable
/// byte pattern appears verbatim in an *unencrypted* payload and cannot in an encrypted one.
fn recognisable() -> Vec<i16> {
    // 8000 is well inside range and encodes to one repeated µ-law code.
    vec![8000i16; 8000]
}

fn keys() -> (SrtpKeys, SrtpKeys) {
    let (a_key, a_salt) = (vec![0x11; 16], vec![0x22; 14]);
    let (b_key, b_salt) = (vec![0x33; 16], vec![0x44; 14]);
    (
        SrtpKeys {
            local: (a_key.clone(), a_salt.clone()),
            remote: (b_key.clone(), b_salt.clone()),
        },
        SrtpKeys {
            local: (b_key, b_salt),
            remote: (a_key, a_salt),
        },
    )
}

/// Two sessions on loopback, optionally encrypted.
async fn pair(srtp: bool) -> (MediaSession, MediaSession) {
    let one = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let two = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let (one_addr, two_addr) = (one.local_addr(), two.local_addr());
    let (one_keys, two_keys) = keys();

    let mut config_one = Config::new(two_addr, Codec::Pcmu);
    config_one.rtcp_interval = None;
    let mut config_two = Config::new(one_addr, Codec::Pcmu);
    config_two.rtcp_interval = None;
    if srtp {
        config_one.srtp = Some(one_keys);
        config_two.srtp = Some(two_keys);
    }

    (one.start(config_one), two.start(config_two))
}

/// M-14's exit criterion. What leaves the socket must not be the audio that went in.
#[tokio::test]
async fn media_on_a_secure_call_is_not_readable_from_the_wire() {
    // A tap standing where anyone on the path stands.
    let tap = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let tap_addr = tap.local_addr().expect("has an address");

    let port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let (local_keys, _) = keys();
    let mut config = Config::new(tap_addr, Codec::Pcmu);
    config.rtcp_interval = None;
    config.srtp = Some(local_keys);
    let session = port.start(config);

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

    // The header is readable — a relay needs it, and RFC 3711 leaves it in the clear.
    assert_eq!(seen[0] >> 6, 2, "still an RTP packet");
    assert_eq!(
        seen[1] & 0x7F,
        Codec::Pcmu.payload_type(),
        "payload type is visible"
    );

    // The payload is not. Unencrypted, every octet after the 12-byte header would be the same
    // µ-law code; encrypted, a run of twenty of them cannot survive.
    let payload = &seen[12..len - 10];
    assert!(
        !payload
            .windows(20)
            .any(|w| w.iter().all(|b| *b == expected_code)),
        "the audio is readable on the wire: found a run of the plaintext µ-law code"
    );
    // And the tag is there, which is what makes the header unforgeable.
    assert_eq!(len, 12 + 160 + 10, "header, encrypted payload, 80-bit tag");

    playing.await.expect("finishes").stop();
}

/// The other half: encrypted media still *is* the audio at the far end.
#[tokio::test]
async fn an_encrypted_call_still_carries_the_audio() {
    let (alice, bob) = pair(true).await;

    let audio = recognisable();
    let (_played, heard) = tokio::join!(
        alice.play(&audio, 160),
        bob.record_until_idle(Duration::from_millis(400)),
    );

    assert!(
        heard.len() > audio.len() / 2,
        "most of the audio should arrive: {} of {}",
        heard.len(),
        audio.len()
    );
    // µ-law round-trips exactly for a value already on its grid, so this is bit-for-bit.
    let expected = sipx_audio::g711::ulaw_decode(sipx_audio::g711::ulaw_encode(8000));
    assert!(
        heard.iter().all(|s| *s == expected),
        "the decrypted audio is not what was sent"
    );

    alice.stop();
    bob.stop();
}

/// A session expecting SRTP must not accept plain RTP. Otherwise an attacker downgrades the call
/// by sending one unencrypted packet, and the encryption becomes decorative.
#[tokio::test]
async fn a_session_expecting_srtp_refuses_plain_rtp() {
    let peer = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let peer_addr = peer.local_addr().expect("has an address");
    let port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let session_addr = port.local_addr();

    let (local_keys, _) = keys();
    let mut config = Config::new(peer_addr, Codec::Pcmu);
    config.rtcp_interval = None;
    config.srtp = Some(local_keys);
    let session = port.start(config);

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
        "a session expecting SRTP accepted unencrypted media"
    );

    session.stop();
}

/// And a packet encrypted under the wrong key is refused rather than played as noise. A decoder
/// pushed past a failed authentication produces a burst that is louder than silence.
#[tokio::test]
async fn a_packet_under_the_wrong_key_is_refused() {
    let peer = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let peer_addr = peer.local_addr().expect("has an address");
    let port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let session_addr = port.local_addr();

    let (local_keys, _) = keys();
    let mut config = Config::new(peer_addr, Codec::Pcmu);
    config.rtcp_interval = None;
    config.srtp = Some(local_keys);
    let session = port.start(config);

    // Encrypted, but by somebody else.
    let mut stranger = sipx_rtp::SrtpContext::new(&[0xEE; 16], &[0xDD; 14]).expect("a context");
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
    assert!(heard.is_err(), "a packet under the wrong key was played");

    session.stop();
}

// -----------------------------------------------------------------------------------------------
// Turning an SDES answer into keys (RFC 4568 §5.1.3, `docs/specs/srtp.md` §5.4).
// -----------------------------------------------------------------------------------------------

fn offered(tag: u32) -> sipx_sdp::crypto::Crypto {
    sipx_sdp::crypto::Crypto::offer(tag, sipx_sdp::crypto::Suite::AesCm128HmacSha1_80, true)
        .expect("secure signalling")
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
    let (alice, bob) = pair(false).await;
    let audio = recognisable();
    let (_played, heard) = tokio::join!(
        alice.play(&audio, 160),
        bob.record_until_idle(Duration::from_millis(400)),
    );
    assert!(heard.len() > audio.len() / 2, "{} samples", heard.len());
    alice.stop();
    bob.stop();
}
