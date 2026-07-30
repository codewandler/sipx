//! Call quality, measured against loss that the test injected itself.
//!
//! The point of asserting against *injected* loss is that a statistics path can be wrong in a
//! way that still produces plausible numbers — a fraction computed over the wrong window, a
//! counter that never resets — and plausible numbers are the ones people trust.

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
use sipx_rtp::rtcp::{ReportBlock, Rtcp};
use tokio::net::UdpSocket;

/// A session listening on a loopback port, and a raw socket standing in for the far end.
async fn session_and_peer() -> (MediaSession, UdpSocket, SocketAddr) {
    let peer = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let peer_addr = peer.local_addr().expect("has an address");

    let port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let session_addr = port.local_addr();

    let mut config = Config::new(peer_addr, Codec::Pcmu);
    config.rtcp_interval = Some(Duration::from_millis(100));
    (port.start(config), peer, session_addr)
}

/// Wait until something has happened, rather than sleeping and assuming it has (`X-29`).
///
/// `within` is a **bound on failure** — how long before we conclude the thing is never going to
/// happen — and not a window to measure in, so it is set orders of magnitude above the honest
/// answer. `X-28` gave a *quantity* of audio its counted form of this; these tests wait on an
/// *event*, so the shape is a deadline loop on the condition instead.
async fn until(within: Duration, what: &str, mut condition: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + within;
    while !condition().await {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// How long these tests wait for hand-sent packets to reach the statistics before calling them
/// lost. Two orders of magnitude above the honest answer on an idle machine.
const ARRIVAL_BOUND: Duration = Duration::from_secs(10);

fn packet(sequence: u16) -> Bytes {
    Packet::new(
        Codec::Pcmu.payload_type(),
        sequence,
        u32::from(sequence) * 160,
        0x1234_5678,
        Bytes::from(vec![0xFFu8; 160]),
    )
    .encode()
}

/// M-10's exit criterion. Twenty packets with every fifth withheld: the statistics must report
/// that loss and not some other number.
#[tokio::test]
async fn statistics_report_the_loss_that_was_actually_injected() {
    let (session, peer, session_addr) = session_and_peer().await;

    let mut sent = 0u64;
    for sequence in 1..=20u16 {
        if sequence % 5 == 0 {
            continue;
        }
        peer.send_to(&packet(sequence), session_addr)
            .await
            .expect("sends");
        sent += 1;
        // Pacing, not a wait: the packets are spaced so they arrive as a stream rather than as
        // one burst. Load lengthens the spacing, which changes nothing this test asserts.
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(sent, 16);

    // Wait for the receive loop to have drained what it is holding, rather than assuming a
    // fixed window was enough (`X-29`). The count is the event: until all sixteen have been
    // through the receive path, the loss below is a partial answer that a slow machine turns
    // into a wrong one.
    until(
        ARRIVAL_BOUND,
        "the sixteen packets never reached the receive path",
        async || session.packets_received() == sent,
    )
    .await;

    let quality = session.quality().await;
    // Three, not four. Sequences 5, 10, 15 and 20 were withheld, but the twentieth is the last
    // one there would have been: a receiver cannot tell a packet that never arrived at the end
    // of a stream from a stream that ended. Only the gaps *inside* what was received are loss,
    // and expecting four here would be expecting the statistics to know the future.
    assert_eq!(
        quality.cumulative_lost, 3,
        "three gaps inside the stream: {quality:?}"
    );
    assert!(
        (quality.loss - 3.0 / 19.0).abs() < 0.03,
        "three of the nineteen expected; reported {}",
        quality.loss
    );
    session.stop();
}

/// A clean stream reports no loss. Without this the test above passes just as happily against
/// an implementation that reports loss unconditionally.
#[tokio::test]
async fn a_clean_stream_reports_no_loss() {
    let (session, peer, session_addr) = session_and_peer().await;

    for sequence in 1..=20u16 {
        peer.send_to(&packet(sequence), session_addr)
            .await
            .expect("sends");
        // Pacing, as above.
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    until(
        ARRIVAL_BOUND,
        "the twenty packets never reached the receive path",
        async || session.packets_received() == 20,
    )
    .await;

    let quality = session.quality().await;
    assert_eq!(quality.cumulative_lost, 0, "{quality:?}");
    assert!(quality.loss < 0.01, "{quality:?}");
    assert!(
        quality.mos > 4.0,
        "a clean stream should score well: {quality:?}"
    );
    session.stop();
}

/// The round trip is `None` rather than zero when nothing has come back. Zero would read as
/// "instantaneous", which is a claim; `None` is the truth.
#[tokio::test]
async fn the_round_trip_is_absent_until_a_report_comes_back() {
    let (session, peer, session_addr) = session_and_peer().await;
    peer.send_to(&packet(1), session_addr).await.expect("sends");
    // A fixed window, deliberately (`X-29`). The assertion below is *negative* — that nothing
    // came back — so a window can only make it pass, and load makes it longer rather than
    // shorter. The failure mode is a missed regression, not a flake; there is no arrival to
    // wait for, and waiting for one that must never come would be a ten-second sleep.
    tokio::time::sleep(Duration::from_millis(150)).await;

    let quality = session.quality().await;
    assert!(
        quality.round_trip.is_none(),
        "nothing has been heard back from a peer that does not do RTCP: {quality:?}"
    );
    session.stop();
}

/// Two real sessions, which is the only way the round-trip calculation can be exercised end to
/// end: it needs a peer that sends sender reports, echoes ours, and reports how long it held
/// them.
#[tokio::test]
async fn two_sessions_measure_the_round_trip_between_them() {
    let one = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let two = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let (one_addr, two_addr) = (one.local_addr(), two.local_addr());

    let mut config_one = Config::new(two_addr, Codec::Pcmu);
    config_one.rtcp_interval = Some(Duration::from_millis(120));
    let mut config_two = Config::new(one_addr, Codec::Pcmu);
    config_two.rtcp_interval = Some(Duration::from_millis(120));

    let one = one.start(config_one);
    let two = two.start(config_two);

    // Both must be sending, or neither sends a sender report and there is no NTP timestamp to
    // echo — which is precisely the case that used to make the round trip unmeasurable.
    let tone: Vec<i16> = (0..16_000).map(|i| ((i % 100) * 100) as i16).collect();
    tokio::join!(one.play(&tone, 160), two.play(&tone, 160));

    let measured = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(trip) = one.quality().await.round_trip {
                return trip;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;

    let trip = measured.expect("a round trip must be measurable between two real sessions");
    // A loose bound on purpose. Whether the peer's own delay is subtracted is not decidable
    // from here — it depends on where in the reporting interval each report happens to land —
    // so it is asserted directly below instead, on the bytes sipx puts on the wire.
    assert!(
        trip < Duration::from_secs(2),
        "loopback is not two seconds away: {trip:?}"
    );

    one.stop();
    two.stop();
}

/// The control port is one above the media port (RFC 3550 §11), and the media port is even.
/// A peer that follows the convention sends its reports there and nowhere else.
#[tokio::test]
async fn the_control_port_sits_directly_above_an_even_media_port() {
    let port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let media = port.local_addr().port();
    assert_eq!(media % 2, 0, "RTP takes the even port of the pair");

    let control: SocketAddr = format!("127.0.0.1:{}", media + 1).parse().expect("valid");
    assert!(
        UdpSocket::bind(control).await.is_err(),
        "the control port must already be held by the session"
    );
}

/// Reading the quality must not disturb what the far end is told.
///
/// `fraction_lost` is computed by *consuming* a reporting window. An implementation that built
/// `quality()` on `report_block()` would have an application polling once a second empty the
/// window the next RTCP report was going to describe — and the far end would be told a lossy
/// call was clean. The bug is invisible until someone displays a live quality meter.
#[tokio::test]
async fn polling_the_quality_does_not_empty_the_report_window() {
    // RTCP off, so the only thing that could consume the window is the polling under test.
    // With the reporting loop running, its own ticks empty it and the test would pass or fail
    // on timing rather than on the thing it names.
    let peer = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let peer_addr = peer.local_addr().expect("has an address");
    let port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let session_addr = port.local_addr();
    let mut config = Config::new(peer_addr, Codec::Pcmu);
    config.rtcp_interval = None;
    let session = port.start(config);

    for sequence in 1..=30u16 {
        if sequence % 3 == 0 {
            continue;
        }
        peer.send_to(&packet(sequence), session_addr)
            .await
            .expect("sends");
        // Poll between every packet, as a live display would.
        let _ = session.quality().await;
        // Pacing, as above.
        tokio::time::sleep(Duration::from_millis(3)).await;
    }
    until(
        ARRIVAL_BOUND,
        "the twenty packets never reached the receive path",
        async || session.packets_received() == 20,
    )
    .await;

    // The report sipx would send still describes the loss.
    let block = session.stats().await;
    assert!(
        block.fraction_lost > 0,
        "the report window was emptied by polling: {block:?}"
    );
    session.stop();
}

/// Reading the statistics must not consume the reporting window either — the failing-first
/// assertion for `M-33`.
///
/// `M-10` fixed this for [`MediaSession::quality`] and left [`MediaSession::stats`] built directly
/// on `StreamStats::report_block`, which *does* consume the window, under a doc comment promising
/// that it does not. Two reads with nothing in between are the sharpest form of the question,
/// because there is exactly one right answer: the second read describes the same interval as the
/// first, since no packet arrived to change it and no report was sent to close it.
#[tokio::test]
async fn reading_the_statistics_twice_reports_the_same_window() {
    // RTCP off, so the only thing that could consume the window is the reading under test.
    // With the reporting loop running, its own ticks close windows and the test would pass or
    // fail on timing rather than on the thing it names.
    let peer = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let peer_addr = peer.local_addr().expect("has an address");
    let port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let session_addr = port.local_addr();
    let mut config = Config::new(peer_addr, Codec::Pcmu);
    config.rtcp_interval = None;
    let session = port.start(config);

    // Ten sequence numbers with two withheld, so the window has loss in it: against an emptied
    // window a clean one is indistinguishable from a correct one.
    inject(&peer, session_addr, &session, &[1, 2, 3, 5, 6, 7, 9, 10], 8).await;

    let first = session.stats().await;
    let second = session.stats().await;

    assert_eq!(
        first.fraction_lost, 51,
        "two of the ten expected, scaled to eight bits: 2 * 256 / 10 = 51 ({first:?})"
    );
    assert_eq!(
        second, first,
        "reading the statistics consumed the reporting window: {first:?} then {second:?}"
    );
    session.stop();
}

/// `fraction_lost` covers the interval since the previous report, and nothing wider.
///
/// RFC 3550 §6.4.1 is normative on the window: the fraction is loss since the previous SR or RR
/// packet was sent. The two intervals below lose *different* fractions on purpose — two in ten,
/// then five in twenty — so a report computed over the whole call would come back 59 rather than
/// 64 and be visibly wrong, where equal fractions would let it pass by coincidence.
#[tokio::test]
async fn fraction_lost_covers_only_the_interval_since_the_previous_report() {
    let (first, second) = two_reporting_intervals(false).await;

    assert_eq!(
        first.fraction_lost, 51,
        "two of the first interval's ten: 2 * 256 / 10 ({first:?})"
    );
    assert_eq!(first.cumulative_lost, 2, "{first:?}");
    assert_eq!(
        second.fraction_lost, 64,
        "five of the second interval's twenty: 5 * 256 / 20. Seven of thirty — the whole call — \
         would be 59, and two of ten would be 51 ({second:?})"
    );
    // The neighbouring field, to show the two cover different windows rather than the same one:
    // cumulative loss spans the call and is the one that says seven.
    assert_eq!(second.cumulative_lost, 7, "{second:?}");
    assert_eq!(second.extended_highest_sequence, 30, "{second:?}");
}

/// An application polling the statistics must not change what the peer is told.
///
/// This is the consequence that makes `M-33` a defect rather than a comment cleanup, and the
/// second assertion that fails while the two comments disagree. A dashboard reading `stats()`
/// between two reports used to close the window the next report was going to describe, so the
/// peer was told a lossy interval was clean — and nothing in sipx's own logs connected the two.
#[tokio::test]
async fn polling_the_statistics_does_not_disturb_the_next_report() {
    let (first, second) = two_reporting_intervals(true).await;

    assert_eq!(first.fraction_lost, 51, "{first:?}");
    assert_eq!(
        second.fraction_lost, 64,
        "a poll between the two reports emptied the window the second was to describe: {second:?}"
    );
    assert_eq!(second.cumulative_lost, 7, "{second:?}");
}

/// The two receiver reports produced by two intervals that lost a known, *different* amount each.
///
/// `poll_between` reads [`MediaSession::stats`] once the second interval's traffic has arrived and
/// before the report describing it goes out, which is what a live quality display does.
async fn two_reporting_intervals(poll_between: bool) -> (ReportBlock, ReportBlock) {
    let (peer_media, peer_control) = peer_with_control_port().await;
    let peer_addr = peer_media.local_addr().expect("has an address");

    let port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let session_addr = port.local_addr();
    let mut config = Config::new(peer_addr, Codec::Pcmu);
    // Two seconds, which is long rather than convenient, and the margin is the point. RFC 3550
    // §6.3.1's randomisation puts a report no sooner than 0.41 of the interval, so the next one
    // is at least 820 ms away, and each batch below is about 70 ms of loopback traffic — an order
    // of magnitude of slack. A short interval would make which window a packet falls into a race,
    // and the exact fractions asserted above would flake rather than fail.
    config.rtcp_interval = Some(Duration::from_secs(2));
    let session = port.start(config);

    // First interval: ten sequence numbers, two withheld.
    inject(
        &peer_media,
        session_addr,
        &session,
        &[1, 2, 3, 5, 6, 7, 9, 10],
        8,
    )
    .await;
    let first = tokio::time::timeout(REPORT_BOUND, next_report(&peer_control))
        .await
        .expect("a first report goes out");

    // Second interval: twenty more with five withheld, a heavier loss than the first.
    let second_batch: Vec<u16> = (11..=30u16)
        .filter(|sequence| !matches!(sequence, 12 | 14 | 16 | 18 | 20))
        .collect();
    inject(&peer_media, session_addr, &session, &second_batch, 23).await;

    if poll_between {
        let polled = session.stats().await;
        assert_eq!(
            polled.cumulative_lost, 7,
            "the poll should see the call as it stands: {polled:?}"
        );
    }

    let second = tokio::time::timeout(REPORT_BOUND, next_report(&peer_control))
        .await
        .expect("a second report goes out");

    session.stop();
    (first, second)
}

/// Send `sequences` to the session and wait until the receive path has counted `total` packets.
///
/// The waiting is not optional (`X-29`): the fractions asserted above are exact, so a packet still
/// in flight when the report goes out does not degrade the answer, it lands in the next window and
/// changes it.
async fn inject(
    peer: &UdpSocket,
    to: SocketAddr,
    session: &MediaSession,
    sequences: &[u16],
    total: u64,
) {
    for &sequence in sequences {
        peer.send_to(&packet(sequence), to).await.expect("sends");
        // Pacing, not a wait: the packets are spaced so they arrive as a stream rather than as
        // one burst. Load lengthens the spacing, which changes nothing this test asserts.
        tokio::time::sleep(Duration::from_millis(3)).await;
    }
    until(
        ARRIVAL_BOUND,
        "the hand-sent packets never reached the receive path",
        async || session.packets_received() == total,
    )
    .await;
}

/// A peer holding a media port and the control port above it (RFC 3550 §11), so it can be sent
/// RTP and receive the reports the session sends back. Retried, because the port above an
/// arbitrary free port is not itself guaranteed to be free.
async fn peer_with_control_port() -> (UdpSocket, UdpSocket) {
    for _ in 0..32 {
        let media = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
        let port = media.local_addr().expect("has an address").port();
        let Some(above) = port.checked_add(1) else {
            continue;
        };
        if let Ok(control) = UdpSocket::bind(format!("127.0.0.1:{above}")).await {
            return (media, control);
        }
    }
    panic!("no adjacent port pair could be bound");
}

/// The next report block the session puts on the wire.
async fn next_report(control: &UdpSocket) -> ReportBlock {
    let mut datagram = vec![0u8; 2048];
    loop {
        let (len, _) = control.recv_from(&mut datagram).await.expect("receives");
        let bytes = Bytes::copy_from_slice(&datagram[..len]);
        let Ok(packets) = Rtcp::decode_compound(&bytes) else {
            continue;
        };
        for packet in packets {
            let blocks = match packet {
                Rtcp::Sender(report) => report.reports,
                Rtcp::Receiver(report) => report.reports,
                _ => continue,
            };
            if let Some(block) = blocks.into_iter().next() {
                return block;
            }
        }
    }
}

/// How long a test here waits for a report that is due within about 1.2 s. A bound on failure.
const REPORT_BOUND: Duration = Duration::from_secs(10);

/// What sipx puts in the report block, checked on the wire.
///
/// The round-trip test above proves the number comes out; this proves sipx holds up its half of
/// the exchange, which is the half the *far end* depends on. Both fields matter and both are
/// easy to leave at zero: without `last_sender_report` the peer has nothing to subtract from,
/// and without `delay_since_last_sender_report` it measures our reporting interval and calls it
/// distance.
#[tokio::test]
async fn our_reports_echo_the_peers_sender_report_and_our_own_delay() {
    use sipx_rtp::rtcp::SenderReport;

    // A peer that owns both ports, so it can send RTP *and* speak RTCP.
    let media_socket = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let media_addr = media_socket.local_addr().expect("has an address");
    let control_socket = UdpSocket::bind(format!("127.0.0.1:{}", media_addr.port() + 1))
        .await
        .expect("the control port is free");

    let port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let session_addr = port.local_addr();
    let mut config = Config::new(media_addr, Codec::Pcmu);
    config.rtcp_interval = Some(Duration::from_millis(150));
    let session = port.start(config);

    // Something to report on.
    for sequence in 1..=5u16 {
        media_socket
            .send_to(&packet(sequence), session_addr)
            .await
            .expect("sends");
    }

    // Our sender report, with a recognisable timestamp.
    let ntp = 0x0000_ABCD_1234_0000u64;
    let expected_echo = sipx_rtp::quality::middle_32(ntp);
    let report = Rtcp::Sender(SenderReport {
        ssrc: 0x1234_5678,
        ntp_timestamp: ntp,
        rtp_timestamp: 800,
        packet_count: 5,
        octet_count: 800,
        reports: Vec::new(),
    });
    let control: SocketAddr = format!("127.0.0.1:{}", session_addr.port() + 1)
        .parse()
        .expect("valid");
    control_socket
        .send_to(&Rtcp::encode_compound(&[report]), control)
        .await
        .expect("sends");

    let mut datagram = vec![0u8; 2048];
    let mut next_block = async || {
        loop {
            let (len, _) = control_socket
                .recv_from(&mut datagram)
                .await
                .expect("receives");
            let bytes = Bytes::copy_from_slice(&datagram[..len]);
            let Ok(packets) = Rtcp::decode_compound(&bytes) else {
                continue;
            };
            for packet in packets {
                let blocks = match packet {
                    Rtcp::Sender(report) => report.reports,
                    Rtcp::Receiver(report) => report.reports,
                    _ => continue,
                };
                if let Some(block) = blocks.into_iter().next() {
                    return block;
                }
            }
        }
    };

    let (first, second) = tokio::time::timeout(Duration::from_secs(10), async {
        let first = next_block().await;
        let second = next_block().await;
        (first, second)
    })
    .await
    .expect("two report blocks come back");

    for block in [&first, &second] {
        assert_eq!(
            block.last_sender_report, expected_echo,
            "the peer's sender report must be echoed, or it has nothing to subtract from"
        );
    }

    // The sharp assertion. Only one sender report was sent, so the delay since it arrived can
    // only grow — and a field left at zero, or filled with a constant, cannot grow. Asserting
    // against the wall clock instead would be asserting on the scheduler.
    assert!(
        second.delay_since_last_sender_report > first.delay_since_last_sender_report,
        "the delay must be a real elapsed time: {} then {}",
        first.delay_since_last_sender_report,
        second.delay_since_last_sender_report
    );
    assert!(
        first.delay_since_last_sender_report > 0,
        "some time passed between the report arriving and our answer"
    );

    session.stop();
}
