//! An RTP header extension survives the relay path (`M-79`).
//!
//! `M-75` taught the packet layer to keep the extension it decoded and to write it back on
//! encode. That is one half of a forwarding path; the other half is the seam between a packet and
//! a relay, and a relay that hands the payload on without the extension delivers media the far end
//! reads differently from the way its sender meant it.
//!
//! Every assertion here is on the far side's **wire bytes** rather than on a session's own types,
//! because that is where the property lives: what leaves the socket is what the far end will act
//! on.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss
)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use sipx_media::{Bridge, Codec, Conference, Config, Encoded, MediaPort, MediaSession};
use sipx_rtp::Packet;
use tokio::net::UdpSocket;

/// How long a test here waits for a packet to come out the far side before calling it lost
/// (`X-28`).
///
/// A bound on failure rather than a window to measure in: a relayed packet crosses two loopback
/// sockets and two tasks, so nothing that arrives inside this is late in any sense a test should
/// care about, and a machine with another gate compiling on it reaches the same verdict.
const ARRIVAL_BOUND: Duration = Duration::from_secs(10);

/// One RFC 8285 one-byte-form extension: profile `0xBEDE`, a length of one 32-bit word, then the
/// word. The same shape `crates/sipx-rtp/tests/header_extension.rs` pins at the packet layer, so a
/// failure here is about the relay and not about parsing.
const EXTENSION: [u8; 8] = [0xBE, 0xDE, 0x00, 0x01, 0x10, 0xAA, 0x00, 0x00];

/// The same element data under a **different identifier**, which is what the other leg would have
/// numbered it (`M-82`).
///
/// RFC 8285 §4.2's one-byte form packs an element as `(id << 4) | (len - 1)`, so `0x10` is
/// identifier 1 over one octet and `0x20` is identifier 2 over the same octet. Only the identifier
/// differs from [`EXTENSION`], which is what makes a comparison between the two a comparison of
/// numbering and of nothing else.
const OTHER_NUMBERING: [u8; 8] = [0xBE, 0xDE, 0x00, 0x01, 0x20, 0xAA, 0x00, 0x00];

/// The same shape with the length word wrong: nine words declared, one carried (`M-85`).
///
/// Only a caller can build this. A relayed extension came through `Packet::decode`, which slices
/// exactly `4 + words * 4` octets and refuses a length word that runs past the datagram, so an
/// extension that arrived over the network agrees with itself by construction.
const SELF_INCONSISTENT_EXTENSION: [u8; 8] = [0xBE, 0xDE, 0x00, 0x09, 0x10, 0xAA, 0x00, 0x00];

/// One packet's worth of µ-law, recognisable enough that a test could not pass on an empty
/// payload.
const PAYLOAD: [u8; 160] = [0xD5; 160];

/// A packet as it arrives from a peer that negotiated an extension.
fn arriving(sequence: u16) -> Bytes {
    with_extension(sequence, Bytes::from_static(&PAYLOAD))
}

/// The same, over a payload the caller chose.
fn with_extension(sequence: u16, payload: Bytes) -> Bytes {
    packet(sequence, payload, &EXTENSION)
}

/// The same, over an extension the caller chose, so a test can give the two legs different
/// numbering (`M-82`).
fn numbered(sequence: u16, extension: &'static [u8]) -> Bytes {
    packet(sequence, Bytes::from_static(&PAYLOAD), extension)
}

/// One encoded packet from a peer, over both of the things a test here varies.
fn packet(sequence: u16, payload: Bytes, extension: &'static [u8]) -> Bytes {
    let mut packet = Packet::new(
        Codec::Pcmu.payload_type(),
        sequence,
        u32::from(sequence) * 160,
        0x0BAD_F00D,
        payload,
    );
    packet.extension = Some(Bytes::from_static(extension));
    packet.encode()
}

/// A media session whose far end is a plain socket, so what the session sends can be read as
/// bytes rather than through the session that would decode them.
async fn leg() -> (MediaSession, UdpSocket, SocketAddr) {
    let peer = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("the far end binds");
    let peer_addr = peer.local_addr().expect("the far end has an address");

    let port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("the session binds");
    let session_addr = port.local_addr();

    let mut config = Config::new(peer_addr, Codec::Pcmu);
    // Nothing here asserts on reports, and a session that also sent them would give the far end
    // more datagrams to sort through for no gain.
    config.rtcp_interval = None;
    // Packets are injected by hand rather than as a paced stream, so the buffer must not be
    // holding the one that was sent while it waits for a neighbour that is not coming.
    config.jitter_depth = 1;

    (
        port.start(config).expect("valid media setup"),
        peer,
        session_addr,
    )
}

/// The next RTP packet to arrive on this socket.
async fn next_packet(on: &UdpSocket, what: &str) -> Packet {
    let mut datagram = vec![0u8; 2048];
    let (len, _) = tokio::time::timeout(ARRIVAL_BOUND, on.recv_from(&mut datagram))
        .await
        .unwrap_or_else(|_elapsed| panic!("{what}"))
        .expect("the far end socket receives");
    Packet::decode(&Bytes::copy_from_slice(&datagram[..len])).expect("what arrived is RTP")
}

/// A bridge hands the extension across with the payload it arrived on.
///
/// The exit criterion of `M-79`: before it, `Encoded` held a payload type and bytes, so this
/// arrived at Bob as a packet with no extension at all.
#[tokio::test]
async fn a_bridged_packet_reaches_the_far_side_with_its_extension() {
    let (left, alice, left_addr) = leg().await;
    let (right, bob, _) = leg().await;

    let bridge = Bridge::connect(Arc::new(left), Arc::new(right));
    assert!(
        !bridge.is_transcoding(),
        "two µ-law legs are passed through, which is the path under test"
    );

    // A short burst rather than a single packet: the relay is a chain of two sockets and three
    // tasks, and a test that turned on the very first datagram would be asserting about
    // scheduling rather than about forwarding.
    for sequence in 1..=4 {
        alice
            .send_to(&arriving(sequence), left_addr)
            .await
            .expect("the peer sends");
    }

    let forwarded = next_packet(&bob, "the bridge relayed nothing to the far leg").await;
    assert_eq!(
        forwarded.extension.as_deref(),
        Some(EXTENSION.as_slice()),
        "the bridge delivered the payload without the header extension it arrived with, so the \
         far end reads this media differently from the way its sender meant it"
    );
    assert_eq!(
        forwarded.payload.as_ref(),
        PAYLOAD.as_slice(),
        "the payload boundary moved, so the extension was written over the media rather than \
         before it"
    );

    bridge.close();
}

/// The two legs number the same element differently, and the bridge translates neither (`M-82`).
///
/// **This pins the decision rather than proving a fix.** It passes on the commit before `M-82` as
/// well as after it, because `M-82` settled what the bridge already did rather than changing it.
/// What it stops is the change nobody would notice going in: a later reading of the elements that
/// renumbers one leg's identifiers into the other's. `docs/specs/media-runtime.md` §5 is the
/// decision and the argument for it.
///
/// The disagreement is the point. Alice's session numbered this element 1 and Bob's numbered it 2,
/// each for itself under RFC 8285 §7 — and sipx negotiated no `a=extmap` on either leg, so it holds
/// no mapping that could relate the two numbers and cannot even see that they differ. Each far end
/// is therefore handed the identifier its own sender wrote, which is the only rule that stays right
/// for an RFC 3550 §5.3.1 extension whose 16-bit profile field is scoped by the profile rather than
/// by a session.
///
/// Both directions at once, which the single-direction `M-79` test does not reach: a renumbering
/// bridge would have to get *one* of them wrong.
#[tokio::test]
async fn neither_leg_is_renumbered_when_the_two_disagree_about_an_identifier() {
    let (left, alice, left_addr) = leg().await;
    let (right, bob, right_addr) = leg().await;

    let bridge = Bridge::connect(Arc::new(left), Arc::new(right));
    assert!(
        !bridge.is_transcoding(),
        "two µ-law legs are passed through, which is the path under test"
    );

    // A short burst in each direction, for the reason the neighbouring test gives: the relay is a
    // chain of sockets and tasks, and turning on the very first datagram would assert about
    // scheduling.
    for sequence in 1..=4 {
        alice
            .send_to(&numbered(sequence, &EXTENSION), left_addr)
            .await
            .expect("Alice sends");
        bob.send_to(&numbered(sequence, &OTHER_NUMBERING), right_addr)
            .await
            .expect("Bob sends");
    }

    let at_bob = next_packet(&bob, "the bridge relayed nothing towards Bob").await;
    assert_eq!(
        at_bob.extension.as_deref(),
        Some(EXTENSION.as_slice()),
        "Bob was handed an identifier Alice did not write, so the bridge invented a reading of an \
         element neither leg negotiated a meaning for"
    );
    assert_eq!(
        at_bob.payload.as_ref(),
        PAYLOAD.as_slice(),
        "the payload boundary moved on the way to Bob"
    );

    let at_alice = next_packet(&alice, "the bridge relayed nothing towards Alice").await;
    assert_eq!(
        at_alice.extension.as_deref(),
        Some(OTHER_NUMBERING.as_slice()),
        "Alice was handed an identifier Bob did not write, so the reverse direction renumbers"
    );
    assert_eq!(
        at_alice.payload.as_ref(),
        PAYLOAD.as_slice(),
        "the payload boundary moved on the way to Alice"
    );

    // The two assertions above are also what says nothing of this endpoint's own rode along: each
    // extension is exactly its sender's eight octets and nothing more. sipx originated no
    // `a=extmap` on either leg, so under RFC 8285 §7 it has no identifier it may send, and the
    // forwarded element being the only element on the packet is why the two numberings never have
    // to be told apart.

    bridge.close();
}

/// One source fanned out to several destinations, and the extension reaches each of them.
///
/// The shape a conference has and a bridge does not: the relayed value is cloned once per
/// destination, so an extension that survived only because a single consumer moved the original
/// would still fail here.
#[tokio::test]
async fn a_fan_out_carries_the_extension_to_every_destination() {
    let (source, speaker, source_addr) = leg().await;
    let (first, first_peer, _) = leg().await;
    let (second, second_peer, _) = leg().await;
    source.set_relay(true);

    for sequence in 1..=4 {
        speaker
            .send_to(&arriving(sequence), source_addr)
            .await
            .expect("the peer sends");
    }

    let heard = tokio::time::timeout(ARRIVAL_BOUND, source.recv_encoded())
        .await
        .expect("a relayed packet reaches the source's encoded path")
        .expect("the source session is still running");
    assert!(first.send_encoded(heard.clone()).await);
    assert!(second.send_encoded(heard).await);

    for (peer, which) in [(&first_peer, "first"), (&second_peer, "second")] {
        let forwarded =
            next_packet(peer, "a fanned-out packet never reached its destination").await;
        assert_eq!(
            forwarded.extension.as_deref(),
            Some(EXTENSION.as_slice()),
            "the {which} destination was sent the payload without its header extension"
        );
        assert_eq!(forwarded.payload.as_ref(), PAYLOAD.as_slice());
    }

    source.stop();
    first.stop();
    second.stop();
}

/// On a **plain** leg the same self-inconsistent extension costs media rather than secrecy
/// (`M-85`).
///
/// There is nothing to leak here — every octet is in the clear by agreement — so the harm is the
/// boundary alone. The far end reads the payload's start from the same length word this side wrote,
/// so it takes thirty-two octets of µ-law as header and plays a packet four fifths as long as the
/// one that was sent. It is not a rejection and there is nothing for it to log: the packet is
/// well formed by every check a receiver can apply, and only the sender ever knew what the
/// boundary was supposed to be. That is why the answer has to sit on this side of the wire.
///
/// The assertion is the full payload rather than the absent extension, because the payload is what
/// the call is: a fix that dropped the extension and kept the boundary would pass on the metadata
/// and still lose the audio.
#[tokio::test]
async fn a_self_inconsistent_extension_costs_a_plain_leg_no_media() {
    let (session, peer, _) = leg().await;

    let mut encoded = Encoded::new(Codec::Pcmu.payload_type(), Bytes::from_static(&PAYLOAD));
    encoded.extension = Some(Bytes::from_static(&SELF_INCONSISTENT_EXTENSION));
    assert!(
        session.send_encoded(encoded).await,
        "the session is running"
    );

    let arrived = next_packet(&peer, "the frame never reached the far end at all").await;
    assert_eq!(
        arrived.payload.len(),
        PAYLOAD.len(),
        "the far end read {} octets of media as header, because the length word this side wrote \
         says the payload starts later than it does",
        PAYLOAD.len().saturating_sub(arrived.payload.len())
    );
    assert_eq!(arrived.payload.as_ref(), PAYLOAD.as_slice());
    assert_eq!(
        arrived.extension, None,
        "an extension that cannot be written self-consistently was written anyway"
    );

    // The same counter as on an encrypted leg, because it is the same refusal at the same
    // boundary: the difference between the legs is what the mistake would have cost, not what
    // happens to it (`docs/specs/media-runtime.md` §4).
    let counted = session.discard_counts();
    assert_eq!(
        counted.malformed_extensions_dropped, 1,
        "the extension was dropped on a plain leg and nothing counted it"
    );
    assert_eq!(counted.total(), 1, "exactly one counter moved");

    session.stop();
}

/// Half a second of a tone, packetised and carrying an extension on every packet.
fn contributed() -> Vec<Bytes> {
    let samples: Vec<i16> = (0..160 * 25)
        .map(|index| {
            let t = f64::from(index) / 8000.0;
            ((t * 440.0 * std::f64::consts::TAU).sin() * 12000.0) as i16
        })
        .collect();
    sipx_audio::g711::ulaw_encode_all(&samples)
        .chunks(160)
        .enumerate()
        .map(|(index, chunk)| with_extension(index as u16 + 1, Bytes::copy_from_slice(chunk)))
        .collect()
}

/// Read from this socket until a packet carries audible audio, checking every packet on the way
/// for an extension it must not have.
async fn until_audible(on: &UdpSocket, what: &str) {
    let deadline = tokio::time::Instant::now() + ARRIVAL_BOUND;
    loop {
        let packet = next_packet(on, what).await;
        assert_eq!(
            packet.extension, None,
            "a mix carried one contributor's header extension, which attributes that \
             participant's measurement to audio that is mostly somebody else's"
        );
        let loudest = packet
            .payload
            .iter()
            .map(|byte| sipx_audio::g711::ulaw_decode(*byte).saturating_abs())
            .max()
            .unwrap_or(0);
        if loudest > 4000 {
            return;
        }
        assert!(tokio::time::Instant::now() < deadline, "{what}");
    }
}

/// A conference does not carry a contributor's extension onto the mix, and that is the decision
/// rather than an omission (`M-79`).
///
/// A mixer's outbound packet is one this endpoint authored from the sum of the others, on its own
/// SSRC and its own timeline (RFC 3550 §7.1). It is not any contributor's packet, so there is
/// nothing on it for a contributor's extension to describe — and with several contributors there
/// is no rule that would pick whose to attach. The audible assertion is what gives this teeth: the
/// contributor's packets really did reach the mixer, extension and all, and the extension still
/// did not come out the other side.
#[tokio::test]
async fn a_conference_mix_carries_no_contributors_extension() {
    let (speaking, speaker, speaking_addr) = leg().await;
    let (listening, listener, _) = leg().await;
    let (also_listening, also_listener, _) = leg().await;

    let conference = Conference::narrowband().expect("valid conference timing");
    conference.join(Arc::new(speaking)).await;
    conference.join(Arc::new(listening)).await;
    conference.join(Arc::new(also_listening)).await;

    for packet in contributed() {
        speaker
            .send_to(&packet, speaking_addr)
            .await
            .expect("the contributor sends");
    }

    until_audible(&listener, "the mix never reached the first listener").await;
    until_audible(&also_listener, "the mix never reached the second listener").await;

    conference.close().await;
}
