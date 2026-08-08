//! The queue between the receive loop and the application (`docs/specs/media-runtime.md` §4.3).
//!
//! `M-45` characterised the jitter buffer across ten 1 500-packet traces and cleared it: bounded,
//! no ratchet, worst measured hold 515 ms. The delay the field reports describe accumulates after
//! it, in the queue an application reads from — so these tests measure that queue and nothing
//! else. The measurement is deliberately made through the public API an application uses, because
//! the quantity under test *is* what an application sees.

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
use sipx_media::{Codec, Config, MediaPort, MediaSession};
use sipx_rtp::Packet;
use tokio::net::UdpSocket;

/// What §4.3 states an application may fall behind live audio by.
///
/// Spelled out here rather than imported, so this file states the contract instead of restating
/// whatever the code currently computes.
const STATED_BOUND: Duration = Duration::from_millis(200);

/// What the measurement may exceed the bound by without being a bound violation.
///
/// The receive loop carries at most one frame past the queue at the instant the burst ends, and
/// the jitter buffer releases its own residue after the stream stops. Neither is queue depth.
/// Five packetisation intervals at the universal 20 ms is an order of magnitude below the
/// difference this test is looking for, which is seconds.
const SLACK: Duration = Duration::from_millis(100);

/// How long these tests wait for hand-sent packets to reach the receive path before calling them
/// lost. A bound on failure, two orders of magnitude above the honest answer on an idle machine.
const ARRIVAL_BOUND: Duration = Duration::from_secs(10);

/// A definition of silence: how long a hole has to be before the burst has certainly ended.
/// The far end sends nothing after the burst, so any gap this long means the queue is drained.
const STREAM_ENDED: Duration = Duration::from_millis(400);

/// A session listening on a loopback port, and a raw socket standing in for the far end.
async fn session_and_peer(packet_duration: Duration) -> (MediaSession, UdpSocket, SocketAddr) {
    let peer = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let peer_addr = peer.local_addr().expect("has an address");

    let port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let session_addr = port.local_addr();

    let mut config = Config::new(peer_addr, Codec::Pcmu);
    config.packet_duration = packet_duration;
    // Nothing here reads RTCP, and a report loop on a shared runtime is one more thing competing
    // for the single test thread while the burst is in flight.
    config.rtcp_interval = None;
    (
        port.start(config).expect("valid media setup"),
        peer,
        session_addr,
    )
}

/// Wait until something has happened, rather than sleeping and assuming it has (`X-29`).
async fn until(within: Duration, what: &str, mut condition: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + within;
    while !condition().await {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        // Polling interval, not a wait: the assertion above is on the condition and on the
        // deadline, and a longer interval only costs this loop another pass.
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// One µ-law packet whose every sample is `level`.
fn packet(sequence: u16, samples_per_packet: usize, level: i16) -> Bytes {
    let payload = vec![sipx_audio::ulaw_encode(level); samples_per_packet];
    Packet::new(
        Codec::Pcmu.payload_type(),
        sequence,
        u32::from(sequence) * samples_per_packet as u32,
        0x1234_5678,
        Bytes::from(payload),
    )
    .encode()
}

/// Send `count` packets back to back with nothing reading the session: an application that has
/// fallen `count` packetisation intervals behind live audio.
///
/// Returns the audio the far end put on the wire.
async fn burst(
    peer: &UdpSocket,
    session: &MediaSession,
    session_addr: SocketAddr,
    count: u16,
    packet_duration: Duration,
    level: impl Fn(u16) -> i16,
) -> Duration {
    let samples_per_packet = session.samples_per_packet();
    for sequence in 1..=count {
        peer.send_to(
            &packet(sequence, samples_per_packet, level(sequence)),
            session_addr,
        )
        .await
        .expect("sends");
        // Not a wait: it hands the runtime to the receive loop so the datagrams are taken off the
        // socket as they land rather than piling up in the kernel's buffer, which would make this
        // a test of the socket's capacity instead of the queue's.
        tokio::task::yield_now().await;
    }
    until(
        ARRIVAL_BOUND,
        "the burst never reached the receive path",
        async || session.packets_received() == u64::from(count),
    )
    .await;
    packet_duration.saturating_mul(u32::from(count))
}

/// How much audio the session had waiting, in time.
fn queued(session: &MediaSession, heard: &[i16]) -> Duration {
    Duration::from_micros(heard.len() as u64 * 1_000_000 / u64::from(session.audio_rate()))
}

/// `M-76`'s failing-first witness.
///
/// An application that reads slower than real time must settle a *stated* distance behind live
/// audio and stay there. Before this story it settled at the far end of a 256-frame channel —
/// 5.12 seconds at the universal 20 ms packetisation — with no bound in time, no counter and no
/// shed policy, and it stayed there for the rest of the call.
#[tokio::test]
async fn a_slow_reader_settles_at_the_bound_not_at_the_end_of_the_queue() {
    let ptime = Duration::from_millis(20);
    let (session, peer, session_addr) = session_and_peer(ptime).await;

    // Four seconds of audio arrive while the application reads none of it. Under the 256-frame
    // channel every one of them was accepted, so the first frame the application then read was
    // four seconds old and every frame after it was too.
    let sent = burst(&peer, &session, session_addr, 200, ptime, |_| 0).await;
    assert!(sent >= Duration::from_secs(4), "the burst was {sent:?}");

    let heard = session.record_until_idle(STREAM_ENDED).await;
    let waiting = queued(&session, &heard);
    assert!(
        waiting <= STATED_BOUND + SLACK,
        "{sent:?} of audio arrived with nothing reading it and the application was handed \
         {waiting:?} of it; §4.3 bounds what may be waiting at {STATED_BOUND:?}"
    );

    session.stop();
}

/// The bound is a duration, so the same configuration means the same delay at every packetisation.
///
/// A depth counted in frames cannot satisfy this: the 256 frames this queue used to hold were
/// 5.12 seconds at 20 ms and 15.36 seconds at 60 ms, which is the sizing inconsistency `M-45`
/// left noted against the jitter buffer's own depth.
#[tokio::test]
async fn the_bound_is_the_same_time_at_every_packet_duration() {
    for (ptime, count) in [
        (Duration::from_millis(20), 200u16),
        (Duration::from_millis(60), 100),
    ] {
        let (session, peer, session_addr) = session_and_peer(ptime).await;
        let sent = burst(&peer, &session, session_addr, count, ptime, |_| 0).await;
        assert!(sent >= Duration::from_secs(4), "the burst was {sent:?}");

        let heard = session.record_until_idle(STREAM_ENDED).await;
        let waiting = queued(&session, &heard);
        assert!(
            waiting <= STATED_BOUND + SLACK,
            "at a {ptime:?} packetisation, {sent:?} of audio arrived with nothing reading it and \
             the application was handed {waiting:?}; §4.3 bounds it at {STATED_BOUND:?} whatever \
             a packet is worth"
        );
        session.stop();
    }
}

/// The policy is shed **oldest**: what survives an overflow is the newest audio.
///
/// Two markers ride the burst, one near its start and one near its end, distinguished by sign so
/// the surviving one names itself. Under shed-oldest the late marker is delivered and the early
/// one is gone; under shed-newest, or under the backpressure this replaced, it is the other way
/// round or both arrive.
#[tokio::test]
async fn an_overflowing_queue_keeps_the_newest_audio() {
    const EARLY: u16 = 5;
    const COUNT: u16 = 200;
    const LATE: u16 = COUNT - 5;
    const MARKER: i16 = 30_000;

    let ptime = Duration::from_millis(20);
    let (session, peer, session_addr) = session_and_peer(ptime).await;
    burst(
        &peer,
        &session,
        session_addr,
        COUNT,
        ptime,
        |sequence| match sequence {
            EARLY => -MARKER,
            LATE => MARKER,
            _ => 0,
        },
    )
    .await;

    let heard = session.record_until_idle(STREAM_ENDED).await;
    // µ-law round-trips a full-scale sample to within a percent, so the threshold is well clear of
    // both the marker and the silence between them.
    assert!(
        heard.iter().any(|sample| *sample > 20_000),
        "the audio that arrived last was not delivered"
    );
    assert!(
        !heard.iter().any(|sample| *sample < -20_000),
        "audio from the start of the burst was still queued, so the oldest audio is not what the \
         overflow sheds"
    );

    session.stop();
}

/// §4's rule at this queue: audio the far end sent that the application will never hear is a
/// media discard, and a media discard is counted.
///
/// The accounting is asserted whole rather than as "the number went up". Every packet that
/// reached the receive path either became a frame the application read or a frame this queue
/// shed, and nothing else — which is the property that makes the counter worth reading when a
/// call is late and nobody knows which layer to blame.
#[tokio::test]
async fn every_frame_the_queue_could_not_hold_is_counted() {
    const COUNT: u16 = 200;

    let ptime = Duration::from_millis(20);
    let (session, peer, session_addr) = session_and_peer(ptime).await;
    burst(&peer, &session, session_addr, COUNT, ptime, |_| 0).await;

    let heard = session.record_until_idle(STREAM_ENDED).await;
    let delivered = heard.len() / session.samples_per_packet();
    let counts = session.discard_counts();

    assert!(
        counts.inbound_frames_shed > 0,
        "{COUNT} packets arrived into a {:?} queue and none was counted as shed",
        Config::DEFAULT_INBOUND_QUEUE
    );
    assert_eq!(
        delivered as u64 + counts.inbound_frames_shed,
        session.packets_received(),
        "every packet that arrived is either audio the application read or a counted shed; \
         delivered {delivered}, shed {}, received {}",
        counts.inbound_frames_shed,
        session.packets_received()
    );
    // Nothing else in the snapshot moved: this burst is in order, on the negotiated payload type
    // and from one source, so `inbound_frames_shed` is the whole of what it cost.
    assert_eq!(
        counts.total(),
        counts.inbound_frames_shed,
        "an in-order burst should cost nothing but the queue's own bound: {counts:?}"
    );

    session.stop();
}

/// The bound the tests above name is the bound the crate publishes.
#[test]
fn the_stated_bound_is_the_configured_default() {
    assert_eq!(Config::DEFAULT_INBOUND_QUEUE, STATED_BOUND);
}
