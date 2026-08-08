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
use sipx_media::{Codec, Config, Encoded, MediaPort, MediaSession, SrtpKeys};
use sipx_rtp::srtp::Profile;
use tokio::net::UdpSocket;

/// Every profile a call can negotiate. The socket-level tests below run over all of them rather
/// than over the counter-mode one they were written for: what leaves the socket has to be
/// unreadable under each transform, not under the one that happened to ship first (`M-41`).
const EVERY_PROFILE: [Profile; 3] = Profile::STRONGEST_FIRST;

/// How long a test here waits for a clip it played to arrive before calling it lost (`X-28`).
/// A bound on failure rather than a window to measure in — see [`MediaSession::record_at_least`].
const DELIVERY_BOUND: Duration = Duration::from_secs(10);

/// A header extension whose embedded length word claims far more than it carries.
///
/// RFC 3550 §5.3.1's shape: a profile field (`0xBEDE`, RFC 8285's one-byte form), a length in
/// 32-bit words, then that many words. Here the length says 255 words — 1020 octets — and four
/// follow, so the header this describes runs past the end of any packet it is written onto.
const OVERSTATED_EXTENSION: [u8; 8] = [0xBE, 0xDE, 0x00, 0xFF, 0x10, 0xAA, 0x00, 0x00];

/// The same mistake, kept **inside** the packet (`M-85`).
///
/// Nine words — forty octets — declared, and eight carried. On a 160-octet payload the header this
/// describes ends at octet 52 of a 180-octet packet, so nothing runs off the end and every
/// bounds check on the way passes. What it moves is the boundary: the thirty-two octets from 20 to
/// 52 are media, and every reader of this packet counts them as header.
const IN_BOUNDS_OVERSTATED_EXTENSION: [u8; 8] = [0xBE, 0xDE, 0x00, 0x09, 0x10, 0xAA, 0x00, 0x00];

/// One packet's worth of µ-law at a constant amplitude, so a leak would be recognisable.
const PAYLOAD: [u8; 160] = [0xD5; 160];

/// How many identical octets in a row count as a run of plaintext µ-law.
///
/// Sixteen rather than two: ciphertext is uniform, so a window this long repeating one value has a
/// probability around `2^-120` of arising by chance, while a four-octet window would show up in a
/// couple of hundred octets of ciphertext often enough to make the assertion flap. The leak this
/// pins is 32 octets long, which is 17 such windows.
const PLAINTEXT_RUN: usize = 16;

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

/// One [`Encoded`] carrying `extension`, sent over an SRTP leg, and everything the far end sees:
/// the datagram exactly as it left the socket, and the packet that decrypts out of it.
///
/// Nothing here asserts about the extension — the two callers below want different things from the
/// same three facts. The session comes back so its counters can be read against the same frame.
async fn protected_with_extension(
    profile: Profile,
    extension: &'static [u8],
) -> (MediaSession, Vec<u8>, sipx_rtp::Packet) {
    let peer = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let peer_addr = peer.local_addr().expect("has an address");
    let port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");

    let (local_keys, far_keys) = keys(profile);
    let mut config = Config::new(peer_addr, Codec::Pcmu);
    config.rtcp_interval = None;
    config.srtp = Some(local_keys);
    let session = port.start(config).expect("valid media setup");

    // Exactly what a relay hands on, except for the extension.
    let mut encoded = Encoded::new(Codec::Pcmu.payload_type(), Bytes::from_static(&PAYLOAD));
    encoded.extension = Some(Bytes::copy_from_slice(extension));
    assert!(
        session.send_encoded(encoded).await,
        "{profile:?}: the session took the frame"
    );

    let mut datagram = vec![0u8; 2048];
    let (len, _) = tokio::time::timeout(DELIVERY_BOUND, peer.recv_from(&mut datagram))
        .await
        .unwrap_or_else(|_elapsed| {
            panic!("{profile:?}: the frame never reached the far end at all")
        })
        .expect("the far end socket receives");
    datagram.truncate(len);

    let (far_key, far_salt) = far_keys.remote.clone();
    let mut far = sipx_rtp::SrtpContext::new(profile, &far_key, &far_salt).expect("a context");
    let plain = far
        .unprotect(&datagram)
        .expect("the far end can decrypt what arrived");
    let arrived = sipx_rtp::Packet::decode(&Bytes::from(plain)).expect("what arrived is RTP");
    (session, datagram, arrived)
}

/// How many windows of [`PLAINTEXT_RUN`] identical payload octets appear anywhere in a datagram.
fn plaintext_runs(datagram: &[u8]) -> usize {
    datagram
        .windows(PLAINTEXT_RUN)
        .filter(|window| window.iter().all(|octet| *octet == PAYLOAD[0]))
        .count()
}

/// The session's counters once it has finished with the frame — waited for, not slept on (§4.1).
async fn counters_after_the_frame(
    session: &MediaSession,
    profile: Profile,
) -> sipx_media::MediaDiscardCounts {
    let deadline = tokio::time::Instant::now() + DELIVERY_BOUND;
    loop {
        let counts = session.discard_counts();
        if counts.total() > 0 {
            return counts;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{profile:?}: an extension was dropped and no discard counter moved — an operator \
             cannot see that the far end is not being sent metadata it may be relying on"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// The extension `M-81` measured — overstating far enough to run **past** the packet — no longer
/// reaches the transform, and no longer costs the call a packet (`M-85`).
///
/// This is `M-81`'s fixture and `M-81`'s test, retargeted. It asserted the consequence that story
/// could reach: `Packet::encode` wrote the extension verbatim, the header SRTP computed was longer
/// than the whole packet, `protect` refused it, and the packet was dropped and counted as
/// `srtp_protect_failures` instead of vanishing. Correct, and still a lost packet on a live call.
///
/// `M-85` refuses the extension one boundary earlier, at `Packet::encode`, which is why the
/// assertions here are the other way round: the media arrives, the extension does not, and
/// `srtp_protect_failures` stays where it now belongs — at zero, on a branch no caller can reach
/// again. Kept over both fixtures because the far-past and just-inside cases are one defect and
/// one rule, and a fix that only covered the case it was written against would leave the other
/// half of it live.
#[tokio::test]
async fn an_extension_overstating_past_the_packet_costs_no_packet() {
    for profile in EVERY_PROFILE {
        let (session, datagram, arrived) =
            protected_with_extension(profile, &OVERSTATED_EXTENSION).await;

        assert_eq!(
            arrived.payload.as_ref(),
            PAYLOAD.as_slice(),
            "{profile:?}: the media was lost with the extension that could not be written"
        );
        assert_eq!(
            arrived.extension, None,
            "{profile:?}: an extension that cannot be written self-consistently was written anyway"
        );
        assert_eq!(
            plaintext_runs(&datagram),
            0,
            "{profile:?}: payload octets left the socket in the clear on an encrypted leg"
        );

        let counted = counters_after_the_frame(&session, profile).await;
        assert_eq!(
            counted.malformed_extensions_dropped, 1,
            "{profile:?}: the extension that could not be written is counted where §4 says it is"
        );
        assert_eq!(
            counted.srtp_protect_failures, 0,
            "{profile:?}: the transform was handed a packet it could not read, which `M-85` is \
             supposed to have made impossible from public API"
        );
        assert_eq!(
            counted.total(),
            1,
            "{profile:?}: one dropped extension moves exactly one counter"
        );

        session.stop();
    }
}

/// A header extension that disagrees with itself puts **no** plaintext media on the wire (`M-85`).
///
/// The in-bounds half of what `M-81` found, and the worse half. An extension whose length word
/// overstates its bytes far enough to run past the packet is refused by the transform and counted;
/// one that overstates them and still lands inside the packet is refused by nothing. The header
/// SRTP computes reaches 32 octets into the payload, and a header is exactly what a header is not
/// encrypted: authenticated but sent in clear under counter mode, Associated Data under the AEAD
/// profiles (RFC 7714 §8.2). Those 32 octets of µ-law leave the socket readable by anyone on the
/// path, on a leg the far end negotiated encryption for.
///
/// Asserted on the datagram rather than on a counter, because the bytes are the defect: a counter
/// that rose while the run still went out would be a worse outcome than no counter at all. The
/// second half of the test decrypts what did arrive, because the promise is that the *media*
/// survives and only the extension is dropped — a fix that silently ate the packet would satisfy
/// the leak assertion and lose the call.
#[tokio::test]
async fn a_self_inconsistent_extension_puts_no_plaintext_media_on_the_wire() {
    for profile in EVERY_PROFILE {
        let (session, datagram, arrived) =
            protected_with_extension(profile, &IN_BOUNDS_OVERSTATED_EXTENSION).await;

        let runs = plaintext_runs(&datagram);
        assert_eq!(
            runs,
            0,
            "{profile:?}: {runs} window(s) of {PLAINTEXT_RUN} identical payload octets left the \
             socket in the clear on an encrypted leg — the extension's length word moved the \
             header boundary into the media (len={})",
            datagram.len()
        );

        // And the call survived the refusal: the media is there, encrypted, with the extension
        // that could not be written left off it.
        assert_eq!(
            arrived.payload.as_ref(),
            PAYLOAD.as_slice(),
            "{profile:?}: the payload the far end reads is not the one that was sent"
        );
        assert_eq!(
            arrived.extension, None,
            "{profile:?}: an extension that cannot be written self-consistently was written anyway"
        );

        let counted = counters_after_the_frame(&session, profile).await;
        assert_eq!(
            counted.malformed_extensions_dropped, 1,
            "{profile:?}: the extension that could not be written is counted where §4 says it is"
        );
        assert_eq!(
            counted.total(),
            1,
            "{profile:?}: one dropped extension moves exactly one counter"
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
