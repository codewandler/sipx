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
use sipx_media::{Bridge, Codec, Conference, Config, MediaPort, MediaSession};
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

/// One packet's worth of µ-law, recognisable enough that a test could not pass on an empty
/// payload.
const PAYLOAD: [u8; 160] = [0xD5; 160];

/// A packet as it arrives from a peer that negotiated an extension.
fn arriving(sequence: u16) -> Bytes {
    with_extension(sequence, Bytes::from_static(&PAYLOAD))
}

/// The same, over a payload the caller chose.
fn with_extension(sequence: u16, payload: Bytes) -> Bytes {
    let mut packet = Packet::new(
        Codec::Pcmu.payload_type(),
        sequence,
        u32::from(sequence) * 160,
        0x0BAD_F00D,
        payload,
    );
    packet.extension = Some(Bytes::from_static(&EXTENSION));
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
