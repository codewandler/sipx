//! `protect` still refuses a header it cannot read — and nothing `encode` produces is one (`M-90`).
//!
//! These are the two halves of one claim, and the claim is what `M-90` published in place of a
//! counter. `docs/specs/media-runtime.md` §4 says the media send loop's SRTP protect-error branch
//! carries a reason rather than a field permanently stuck at zero, and the reason is that
//! `Packet::encode` cannot build a packet whose header `SrtpContext::protect` cannot read.
//!
//! `M-81` is why this is a test rather than a sentence. That story's own lesson — *"unreachable
//! because of what the caller can be" is a claim with an expiry date* — was learned when `M-79`
//! made `Encoded::extension` public and turned an unreachable branch into a reachable one without
//! either site changing. A prose reason expires in silence. This fails.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use bytes::Bytes;
use sipx_rtp::packet::Packet;
use sipx_rtp::srtp::Profile;
use sipx_rtp::{SrtpContext, SrtpError};

/// Every profile a call can negotiate: the guard has to hold under each transform, not under the
/// one a test happened to be written against.
const EVERY_PROFILE: [Profile; 3] = Profile::STRONGEST_FIRST;

/// A context keyed for `profile`, from key material of exactly the length it requires.
fn context(profile: Profile) -> SrtpContext {
    let (key_len, salt_len) = profile.key_and_salt_len();
    let key: Vec<u8> = (0..key_len).map(|i| i as u8).collect();
    let salt: Vec<u8> = (0..salt_len).map(|i| 0x80 ^ i as u8).collect();
    SrtpContext::new(profile, &key, &salt).expect("the profile's own key and salt lengths")
}

/// One packet's worth of µ-law, before anything adversarial is done to its header.
fn base() -> Packet {
    Packet::new(0, 0x2A, 0x64, 0x0000_0007, Bytes::from_static(&[0xD5; 160]))
}

/// A named case: what `build` does to a well-formed packet is what is adversarial about it.
fn case(name: &'static str, build: impl FnOnce(&mut Packet)) -> (&'static str, Packet) {
    let mut packet = base();
    build(&mut packet);
    (name, packet)
}

/// The packets a caller can reach `protect` with, named by what is wrong with each.
///
/// Every one is built through `Packet`'s public fields, which is the whole surface
/// `MediaSession::send_encoded` exposes: a payload, and an extension carried verbatim off an
/// `Encoded`. The CSRC cases are here because the count travels in a four-bit nibble and the list
/// does not, so a list longer than the nibble can name is the other way the header a writer emits
/// could stop agreeing with the header a reader computes.
fn adversarial_packets() -> Vec<(&'static str, Packet)> {
    vec![
        case("a plain packet", |_| {}),
        case("no payload at all", |packet| packet.payload = Bytes::new()),
        // RFC 3550 §5.3.1's shape, satisfied: profile, a length of one 32-bit word, then that word.
        case("an extension that agrees with itself", |packet| {
            packet.extension = Some(Bytes::from_static(&[
                0xBE, 0xDE, 0x00, 0x01, 0x10, 0xAA, 0, 0,
            ]));
        }),
        // `M-81`'s fixture: 255 words declared, one carried. The header this describes runs past
        // the end of any packet it could be written onto — the case that used to reach `protect`.
        case("an extension overstating past the packet", |packet| {
            packet.extension = Some(Bytes::from_static(&[
                0xBE, 0xDE, 0x00, 0xFF, 0x10, 0xAA, 0, 0,
            ]));
        }),
        // `M-85`'s fixture: nine words declared, one carried. It stays inside the packet, which is
        // why every bounds check passed and only the payload boundary moved.
        case("an extension overstating inside the packet", |packet| {
            packet.extension = Some(Bytes::from_static(&[
                0xBE, 0xDE, 0x00, 0x09, 0x10, 0xAA, 0, 0,
            ]));
        }),
        case("an extension understating its bytes", |packet| {
            packet.extension = Some(Bytes::from_static(&[
                0xBE, 0xDE, 0x00, 0x01, 0x10, 0xAA, 0, 0, 0xFF,
            ]));
        }),
        case("an extension with no length word in it", |packet| {
            packet.extension = Some(Bytes::from_static(&[0xBE, 0xDE]));
        }),
        case("an empty extension", |packet| {
            packet.extension = Some(Bytes::new());
        }),
        case("the fifteen CSRCs the nibble can name", |packet| {
            packet.csrc = (0..15).collect();
        }),
        case("more CSRCs than the nibble can name", |packet| {
            packet.csrc = (0..40).collect();
        }),
        case(
            "more CSRCs than the nibble can name, with an extension",
            |packet| {
                packet.csrc = (0..40).collect();
                packet.extension = Some(Bytes::from_static(&[0xBE, 0xDE, 0x00, 0x01, 1, 2, 3, 4]));
            },
        ),
    ]
}

/// **The premise of `M-90`'s removal**: no packet `Packet::encode` produces makes `protect` fail.
///
/// The media send loop protects exactly `packet.encode()` and nothing else, so this is the whole
/// of what a call can put in front of the transform. When this test fails, that loop's
/// protect-error branch has become reachable from a caller again and
/// `docs/specs/media-runtime.md` §4 owes it a counter — this failure being the notice.
#[test]
fn nothing_encode_produces_has_a_header_protect_cannot_read() {
    for profile in EVERY_PROFILE {
        for (name, packet) in adversarial_packets() {
            let encoded = packet.encode();
            let protected = context(profile).protect(&encoded);
            assert!(
                protected.is_ok(),
                "{profile:?}: `encode` produced a packet `protect` refused ({name}: \
                 {protected:?}) — the media send loop's protect-error branch is reachable from a \
                 caller again, and `docs/specs/media-runtime.md` §4 says a reachable discard is \
                 counted rather than only logged"
            );
        }
    }
}

/// The refusal itself is still there — the premise is that nothing reaches it, not that it went.
///
/// A header the transform cannot read is refused rather than protected part-way, under every
/// profile. Removing this guard would put a packet on the wire with its payload boundary in a
/// place only the sender believed in, which is the failure `M-85` measured at the other end of
/// this same path.
#[test]
fn protect_still_refuses_a_header_longer_than_its_packet() {
    // Twelve octets of fixed header with the X bit set, then an extension declaring 255 words and
    // carrying one. Assembled by hand: `Packet::encode` will not write it, which is the point.
    let mut raw = vec![
        0b1001_0000, // version 2, extension bit, no CSRCs
        0x00,        // payload type 0, no marker
        0x00,
        0x2A, // sequence
        0x00,
        0x00,
        0x00,
        0x64, // timestamp
        0x00,
        0x00,
        0x00,
        0x07, // ssrc
        0xBE,
        0xDE,
        0x00,
        0xFF, // extension profile, 255 words declared
        0x10,
        0xAA,
        0x00,
        0x00, // one word carried
    ];
    raw.extend_from_slice(&[0xD5; 160]);

    for profile in EVERY_PROFILE {
        assert_eq!(
            context(profile).protect(&raw),
            Err(SrtpError::TooShort(raw.len())),
            "{profile:?}: a packet whose header runs past its own end was protected rather than \
             refused"
        );
    }
}

/// A packet too short to hold the fixed header is refused too, on the same error.
///
/// `protect`'s second header refusal. `Packet::encode` always writes twelve octets, so this is
/// unreachable from the send loop for the same reason as the first — and pinned here for the same
/// reason.
#[test]
fn protect_still_refuses_a_packet_shorter_than_a_header() {
    for profile in EVERY_PROFILE {
        for len in 0..12usize {
            let short = vec![0x80; len];
            assert_eq!(
                context(profile).protect(&short),
                Err(SrtpError::TooShort(len)),
                "{profile:?}: {len} octets is not an RTP packet and was protected anyway"
            );
        }
    }
}
