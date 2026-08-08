//! The bounded call PCM processing seam (`docs/specs/call-audio-seam.md`, `M-54`).
//!
//! These run against a live `MediaSession` on loopback rather than against the queue in isolation,
//! because the claims worth proving are about the *seam*: that a processor sees the audio the call
//! actually carried, in the format it asked for, and that a processor which stops reading loses its
//! own frames and nobody else's — least of all the call's.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use sipx_media::{
    AudioDirection, Codec, Config, DiscontinuityKind, MediaPort, MediaSession, PcmEncoding,
    PcmFormat, PcmSamples, Processing, ProcessingError,
};
use sipx_rtp::Packet;
use tokio::net::UdpSocket;

/// How long these tests wait for audio to cross loopback before calling it lost.
///
/// A bound on failure, orders of magnitude above the honest answer on an idle machine, and never a
/// window anything is measured in.
const ARRIVAL_BOUND: Duration = Duration::from_secs(10);

const SAMPLES_PER_PACKET: usize = 160;

/// A session on loopback, and a raw socket standing in for the far end.
async fn session_and_peer() -> (MediaSession, UdpSocket, SocketAddr) {
    let peer = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let peer_addr = peer.local_addr().expect("has an address");

    let port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let session_addr = port.local_addr();

    let mut config = Config::new(peer_addr, Codec::Pcmu);
    config.rtcp_interval = None;
    (
        port.start(config).expect("valid media setup"),
        peer,
        session_addr,
    )
}

/// One µ-law packet of a constant level, so a decoded frame is recognisable by value.
fn packet(sequence: u16, level: u8) -> Bytes {
    Packet::new(
        Codec::Pcmu.payload_type(),
        sequence,
        u32::from(sequence) * SAMPLES_PER_PACKET as u32,
        0x2c54_0001,
        Bytes::from(vec![level; SAMPLES_PER_PACKET]),
    )
    .encode()
}

async fn feed(peer: &UdpSocket, to: SocketAddr, packets: u16, level: u8) {
    for sequence in 0..packets {
        peer.send_to(&packet(sequence, level), to)
            .await
            .expect("sends");
    }
}

fn signed(samples: &PcmSamples) -> &[i16] {
    match samples {
        PcmSamples::Signed16(samples) => samples,
        other => panic!("expected signed samples, got {other:?}"),
    }
}

fn narrowband() -> PcmFormat {
    PcmFormat::new(8_000, PcmEncoding::Signed16).expect("a supported format")
}

/// Acceptance row 1: both taps exist, and a frame places itself on a timeline.
#[tokio::test]
async fn attached_processors_observe_both_directions_with_frame_metadata() {
    let (session, peer, session_addr) = session_and_peer().await;

    let mut received = session
        .attach_processor(Processing::new(AudioDirection::Inbound, narrowband()))
        .expect("attaches to received audio");
    let mut transmitted = session
        .attach_processor(Processing::new(AudioDirection::Outbound, narrowband()))
        .expect("attaches to transmitted audio");

    assert_eq!(received.direction(), AudioDirection::Inbound);
    assert_eq!(transmitted.direction(), AudioDirection::Outbound);
    assert_eq!(received.format(), narrowband());

    // Received: four packets clear the jitter buffer's depth without waiting for its flush.
    feed(&peer, session_addr, 4, 0xFF).await;
    let first = tokio::time::timeout(ARRIVAL_BOUND, received.recv())
        .await
        .expect("received audio reaches the seam")
        .expect("a frame");

    assert_eq!(first.direction(), AudioDirection::Inbound);
    assert_eq!(first.format(), narrowband());
    assert_eq!(first.sequence(), 0);
    assert_eq!(first.sample_time(), 0);
    assert_eq!(first.discontinuity(), None);
    assert_eq!(signed(first.pcm().samples()).len(), SAMPLES_PER_PACKET);

    let second = tokio::time::timeout(ARRIVAL_BOUND, received.recv())
        .await
        .expect("the stream continues")
        .expect("a second frame");
    assert_eq!(second.sequence(), 1);
    assert_eq!(second.sample_time(), SAMPLES_PER_PACKET as u64);
    assert_eq!(second.discontinuity(), None);

    // Transmitted: what this side put on the wire, after the mute gate and before encoding.
    let tone: Vec<i16> = (0..i16::try_from(SAMPLES_PER_PACKET).expect("a packet fits an i16"))
        .map(|index| index * 64)
        .collect();
    assert!(session.send(tone.clone()).await, "queues outbound audio");
    let sent = tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
        .await
        .expect("transmitted audio reaches the seam")
        .expect("a frame");

    assert_eq!(sent.direction(), AudioDirection::Outbound);
    assert_eq!(sent.sequence(), 0);
    assert_eq!(sent.sample_time(), 0);
    assert_eq!(sent.discontinuity(), None);
    assert_eq!(signed(sent.pcm().samples()), tone.as_slice());

    session.shutdown().await;
}

/// Acceptance row 2: the format is the processor's choice, and an impossible one is a typed
/// refusal rather than a distorted or dropped call.
#[tokio::test]
async fn an_unsupported_conversion_is_refused_by_type() {
    let (session, peer, session_addr) = session_and_peer().await;

    for rate in [0, 384_001] {
        let refusal = PcmFormat::new(rate, PcmEncoding::Signed16)
            .map(|format| Processing::new(AudioDirection::Inbound, format))
            .map_err(ProcessingError::UnsupportedConversion)
            .and_then(|request| session.attach_processor(request))
            .expect_err("an unsupported rate is refused");
        assert!(
            matches!(refusal, ProcessingError::UnsupportedConversion(_)),
            "{rate} Hz: {refusal:?}"
        );
    }

    for capacity in [1, 4_097] {
        let refusal = session
            .attach_processor(
                Processing::new(AudioDirection::Inbound, narrowband())
                    .with_queue_capacity(capacity),
            )
            .expect_err("an out-of-domain capacity is refused");
        assert!(
            matches!(refusal, ProcessingError::QueueCapacity { .. }),
            "{capacity}: {refusal:?}"
        );
    }

    // A refused attachment leaves the call carrying audio.
    feed(&peer, session_addr, 4, 0xFF).await;
    let audio = tokio::time::timeout(ARRIVAL_BOUND, session.recv())
        .await
        .expect("the call still carries audio")
        .expect("a frame");
    assert_eq!(audio.len(), SAMPLES_PER_PACKET);

    session.shutdown().await;
}

/// Acceptance row 2, the other half: a processor may ask for a rate the session does not run at,
/// and gets it through `M-43`'s resampler rather than through a seam-local copy.
#[tokio::test]
async fn a_processor_receives_its_own_requested_rate() {
    let (session, peer, session_addr) = session_and_peer().await;
    let wideband = PcmFormat::new(16_000, PcmEncoding::Signed16).expect("supported");
    let mut processor = session
        .attach_processor(Processing::new(AudioDirection::Inbound, wideband))
        .expect("attaches");

    feed(&peer, session_addr, 4, 0xFF).await;
    let first = tokio::time::timeout(ARRIVAL_BOUND, processor.recv())
        .await
        .expect("audio reaches the seam")
        .expect("a frame");
    assert_eq!(first.format(), wideband);
    let first_len = signed(first.pcm().samples()).len();
    assert!(
        first_len > SAMPLES_PER_PACKET,
        "8 kHz to 16 kHz doubles the sample count, got {first_len}"
    );

    let second = tokio::time::timeout(ARRIVAL_BOUND, processor.recv())
        .await
        .expect("the stream continues")
        .expect("a second frame");
    assert_eq!(
        second.sample_time(),
        first_len as u64,
        "the epoch advances by what was delivered"
    );

    session.shutdown().await;
}

/// Acceptance row 3: a processor that stops reading loses its own oldest frames, is told so, and
/// does not stall the call.
#[tokio::test]
async fn a_slow_processor_loses_the_oldest_frames_and_is_told() {
    let (session, peer, session_addr) = session_and_peer().await;
    let mut slow = session
        .attach_processor(
            Processing::new(AudioDirection::Inbound, narrowband()).with_queue_capacity(2),
        )
        .expect("attaches");

    feed(&peer, session_addr, 8, 0xFF).await;

    // The call is not blocked by the unread attachment: every packet still reaches the
    // application receive path.
    let mut delivered = 0usize;
    while delivered < 8 {
        let frame = tokio::time::timeout(ARRIVAL_BOUND, session.recv())
            .await
            .expect("the call is never blocked by a slow processor")
            .expect("a frame");
        assert_eq!(frame.len(), SAMPLES_PER_PACKET);
        delivered += 1;
    }

    let first = tokio::time::timeout(ARRIVAL_BOUND, slow.recv())
        .await
        .expect("the slow processor still gets what its queue held")
        .expect("a frame");
    let discontinuity = first
        .discontinuity()
        .expect("the frame after the gap names the gap");
    assert_eq!(discontinuity.kind(), DiscontinuityKind::Overflow);
    assert_eq!(discontinuity.frames(), 6);
    assert_eq!(discontinuity.samples(), 6 * SAMPLES_PER_PACKET as u64);
    assert_eq!(first.sequence(), 6, "the sequence gap is what was lost");
    assert_eq!(
        first.sample_time(),
        6 * SAMPLES_PER_PACKET as u64,
        "the timeline does not compress over loss"
    );

    assert_eq!(slow.lost_frames(), 6);
    assert_eq!(session.discard_counts().processor_frames_lost, 6);

    session.shutdown().await;
}

/// Acceptance row 4: attach, detach, cancellation and abandonment all release the attachment, and
/// completion is an event rather than an elapsed duration.
#[tokio::test]
async fn attachments_are_released_with_observable_completion() {
    let (session, peer, session_addr) = session_and_peer().await;

    // Detached explicitly: the call carries on and the attachment is gone.
    let detached = session
        .attach_processor(Processing::new(AudioDirection::Inbound, narrowband()))
        .expect("attaches");
    detached.detach();

    // Abandoned: dropped without detaching, which is what a failed processor looks like.
    drop(
        session
            .attach_processor(Processing::new(AudioDirection::Inbound, narrowband()))
            .expect("attaches"),
    );

    let mut live = session
        .attach_processor(Processing::new(AudioDirection::Inbound, narrowband()))
        .expect("attaches");
    feed(&peer, session_addr, 4, 0xFF).await;
    tokio::time::timeout(ARRIVAL_BOUND, live.recv())
        .await
        .expect("audio reaches the surviving attachment")
        .expect("a frame");

    // Cancelling the call completes every attachment after it has drained.
    session.stop();
    let completion = tokio::time::timeout(ARRIVAL_BOUND, async {
        while live.recv().await.is_some() {}
    })
    .await;
    assert!(
        completion.is_ok(),
        "a stopped session completes its attachments"
    );

    assert!(
        session
            .attach_processor(Processing::new(AudioDirection::Inbound, narrowband()))
            .is_err(),
        "a stopped session hands out no new attachment"
    );

    session.shutdown().await;
}

/// Acceptance row 5: two simultaneous consumers, each with its own format, queue and losses.
#[tokio::test]
async fn two_simultaneous_consumers_share_no_mutable_state() {
    let (session, peer, session_addr) = session_and_peer().await;

    // Standing in for a speech provider: wideband, generously queued, read promptly.
    let wideband = PcmFormat::new(16_000, PcmEncoding::Signed16).expect("supported");
    let mut speech = session
        .attach_processor(Processing::new(AudioDirection::Inbound, wideband))
        .expect("attaches");
    // Standing in for a deterministic analyser: the call's own rate, tiny queue, read late.
    let mut analyser = session
        .attach_processor(
            Processing::new(AudioDirection::Inbound, narrowband()).with_queue_capacity(2),
        )
        .expect("attaches");

    feed(&peer, session_addr, 8, 0xFF).await;

    let mut speech_frames = 0usize;
    while speech_frames < 8 {
        let frame = tokio::time::timeout(ARRIVAL_BOUND, speech.recv())
            .await
            .expect("the prompt consumer keeps up")
            .expect("a frame");
        assert_eq!(frame.format(), wideband);
        assert_eq!(
            frame.discontinuity(),
            None,
            "the prompt consumer loses nothing to the slow one"
        );
        speech_frames += 1;
    }
    assert_eq!(speech.lost_frames(), 0);

    let late = tokio::time::timeout(ARRIVAL_BOUND, analyser.recv())
        .await
        .expect("the late consumer still gets its queue")
        .expect("a frame");
    assert_eq!(late.format(), narrowband());
    assert!(
        late.discontinuity().is_some(),
        "the late consumer sees its own loss"
    );
    assert!(analyser.lost_frames() > 0);

    session.shutdown().await;
}

/// SEAM-12: an attachment belongs to the call, so a re-INVITE re-anchors it rather than losing it.
#[tokio::test]
async fn an_attachment_survives_renegotiation_and_is_re_anchored() {
    let (mut session, peer, session_addr) = session_and_peer().await;
    let mut processor = session
        .attach_processor(Processing::new(AudioDirection::Inbound, narrowband()))
        .expect("attaches");

    feed(&peer, session_addr, 4, 0xFF).await;
    let before = tokio::time::timeout(ARRIVAL_BOUND, processor.recv())
        .await
        .expect("audio reaches the seam")
        .expect("a frame");
    assert_eq!(before.discontinuity(), None);

    let mut next = Config::new(peer.local_addr().expect("has an address"), Codec::Pcma);
    next.rtcp_interval = None;
    assert!(
        session.reconfigure(next).await.expect("valid media setup"),
        "a session without ICE reconfigures in place"
    );

    let peer_addr = session.local_addr();
    for sequence in 0..4u16 {
        let packet = Packet::new(
            Codec::Pcma.payload_type(),
            sequence,
            u32::from(sequence) * SAMPLES_PER_PACKET as u32,
            0x2c54_0002,
            Bytes::from(vec![0xD5u8; SAMPLES_PER_PACKET]),
        )
        .encode();
        peer.send_to(&packet, peer_addr).await.expect("sends");
    }

    let after = tokio::time::timeout(ARRIVAL_BOUND, processor.recv())
        .await
        .expect("the attachment survived the renegotiation")
        .expect("a frame");
    let re_anchor = after
        .discontinuity()
        .expect("the new generation opens a new epoch");
    assert_eq!(re_anchor.kind(), DiscontinuityKind::Realign);
    assert_eq!(re_anchor.samples(), 0);
    assert_eq!(after.sample_time(), 0);

    session.shutdown().await;
}

/// §3: a relaying session decodes nothing, so it produces no received frames — a stated absence
/// rather than a silent one.
#[tokio::test]
async fn a_relaying_session_produces_no_received_frames() {
    let (session, peer, session_addr) = session_and_peer().await;
    session.set_relay(true);
    let mut processor = session
        .attach_processor(Processing::new(AudioDirection::Inbound, narrowband()))
        .expect("attaches");

    feed(&peer, session_addr, 4, 0xFF).await;
    let encoded = tokio::time::timeout(ARRIVAL_BOUND, session.recv_encoded())
        .await
        .expect("the relay path still carries the packet")
        .expect("an encoded frame");
    assert_eq!(encoded.payload.len(), SAMPLES_PER_PACKET);

    // The relayed packet has already been delivered, so an empty seam here is a decision and not
    // a race: nothing further is in flight for the processor to receive.
    assert!(processor.try_recv().is_none());

    session.shutdown().await;
}
