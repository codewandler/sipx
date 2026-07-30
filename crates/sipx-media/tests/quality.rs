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
/// `fraction_lost` covers the interval since the last report went out, and it is the *sending*
/// path that closes that interval (`M-33`). An implementation that built `quality()` on
/// `report_block()` would have an application polling once a second close windows nobody was told
/// about — and the far end would be told a lossy call was clean. The bug is invisible until
/// someone displays a live quality meter.
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
/// packet *was sent*. Asserted on every report the session emits, against the interval that report
/// names for itself — see [`reports_hold_to_their_own_intervals`] for why it is derived rather than
/// arranged.
#[tokio::test]
async fn fraction_lost_covers_only_the_interval_since_the_previous_report() {
    reports_hold_to_their_own_intervals(Polling::None).await;
}

/// An application polling the statistics must not change what the peer is told.
///
/// This is the consequence that makes `M-33` a defect rather than a comment cleanup, and the second
/// assertion that fails while the two comments disagree. A dashboard reading `stats()` used to close
/// the window the next report was going to describe, so the peer was told a lossy interval was clean
/// — and nothing in sipx's own logs connected the two. The polling here is per packet, which is
/// heavier than any real dashboard and needs to be: it makes the corruption certain rather than
/// dependent on a poll landing inside the right interval.
#[tokio::test]
async fn polling_the_statistics_does_not_disturb_the_next_report() {
    reports_hold_to_their_own_intervals(Polling::EveryPacket).await;
}

/// Whether a dashboard is reading [`MediaSession::stats`] while the stream runs.
#[derive(Clone, Copy, PartialEq)]
enum Polling {
    None,
    EveryPacket,
}

/// Every report the session sends, held against the interval it says it covers.
///
/// **Why the expectation is derived and not arranged.** The obvious version of this test injects a
/// known loss, waits for a report, injects a different loss and asserts an exact fraction in the
/// second — and it is unreliable, because it assumes the report timer fires between the two batches
/// rather than inside one. That assumption is a happens-before dressed as a duration, which
/// `docs/designs/media.md` forbids for exactly this reason, and lengthening the interval only buys
/// slack: under a 6x CPU oversubscription the paced injection stretches past any margin and the
/// report describes half a batch.
///
/// So nothing here waits on a boundary. The stream is injected with a known set of sequence numbers
/// withheld, and each report is checked against the window it *names*:
/// `extended_highest_sequence` is the last sequence number that report covers, the previous report's
/// is where its window opened, and the test knows which numbers it withheld — so §6.4.1's required
/// fraction is computable for whatever boundary the timer chose. Wherever the reports land, every
/// one of them has to be right about its own interval, and a report computed over the whole call
/// cannot be.
async fn reports_hold_to_their_own_intervals(polling: Polling) {
    let (peer_media, peer_control) = peer_with_control_port().await;
    let peer_addr = peer_media.local_addr().expect("has an address");

    let port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let session_addr = port.local_addr();
    let mut config = Config::new(peer_addr, Codec::Pcmu);
    // Short on purpose, where the arranged version of this test needed a long one: the more
    // boundaries the timer draws, the more intervals get checked, and a boundary landing mid-stream
    // is now a case this test covers rather than one it loses to.
    config.rtcp_interval = Some(Duration::from_millis(120));
    let session = port.start(config);

    let injected: Vec<u16> = (1..=LAST)
        .filter(|sequence| !WITHHELD.contains(sequence))
        .collect();
    for &sequence in &injected {
        peer_media
            .send_to(&packet(sequence), session_addr)
            .await
            .expect("sends");
        if polling == Polling::EveryPacket {
            let _ = session.stats().await;
        }
        // Pacing, and reports fire while it runs — which is the point rather than the hazard it
        // used to be. Load lengthens the spacing and moves the boundaries; the assertions follow
        // the boundaries, so it changes which intervals are checked and not whether they hold.
        tokio::time::sleep(Duration::from_millis(3)).await;
    }
    let counted = u64::try_from(injected.len()).expect("a small count");
    until(
        ARRIVAL_BOUND,
        "the injected packets never reached the receive path",
        async || session.packets_received() == counted,
    )
    .await;

    // Read reports until one covers an *empty* interval. Injection has finished, so a report whose
    // highest sequence number is `LAST` and equal to its predecessor's has nothing left to describe
    // — there is no later boundary to learn anything from, and no timing assumption in saying so.
    let mut opened_at = 0u32;
    let mut reports = 0usize;
    let mut carried_loss = false;
    loop {
        let block = tokio::time::timeout(REPORT_BOUND, next_report(&peer_control))
            .await
            .expect("a report goes out");
        reports += 1;

        let highest = block.extended_highest_sequence;
        let due = fraction_over(opened_at, highest);
        assert_eq!(
            block.fraction_lost,
            due,
            "report {reports} says it covers sequence numbers {} to {highest}, of which this test \
             withheld {} of {} — RFC 3550 §6.4.1 makes that {due} in 256ths: {block:?}",
            opened_at + 1,
            withheld_within(opened_at, highest),
            highest.saturating_sub(opened_at),
        );
        carried_loss |= due > 0;

        if highest == opened_at && highest == u32::from(LAST) {
            // The sharp end, and it takes no arranging: this interval is empty, so a report that
            // describes its interval reports no loss — while one computed over the whole call would
            // report four of forty, which is 25 in 256ths.
            assert_eq!(
                block.fraction_lost, 0,
                "an empty interval lost nothing: {block:?}"
            );
            assert_eq!(
                block.cumulative_lost,
                i32::try_from(WITHHELD.len()).expect("a small count"),
                "and cumulative loss still remembers the whole call: {block:?}"
            );
            break;
        }
        opened_at = highest;
    }

    // Without this a run in which the timer drew no boundary over the lossy prefix would assert a
    // row of zeroes and call it a pass.
    assert!(
        carried_loss,
        "no report carried the injected loss, so nothing above was tested"
    );
    assert!(reports >= 2, "only {reports} report(s) were checked");
    session.stop();
}

/// The sequence numbers withheld from the stream: four packets, all inside the first ten, then a
/// clean run to [`LAST`]. Front-loaded on purpose — the whole-call fraction stays elevated for the
/// rest of the stream, so a report that quotes it instead of its own interval is visibly wrong
/// wherever the boundary happens to fall.
const WITHHELD: [u16; 4] = [2, 4, 6, 8];

/// The last sequence number injected.
const LAST: u16 = 40;

/// What RFC 3550 §6.4.1 requires of a report covering the sequence numbers after `opened_at` up to
/// and including `highest`: the packets withheld in that range over the size of the range, in
/// 256ths, and zero for a range that is empty or lost nothing.
///
/// This is the receiver's own arithmetic restated from the sending side, which is what makes it a
/// check rather than a copy: `expected` is a span of sequence numbers, and every number in the span
/// that this test did not send is loss. It holds because the first packet injected is sequence 1 and
/// arrives, so the receiver's base sequence number is 1 — the `cumulative_lost` assertion above
/// fails loudly if the fixture's own loopback ever drops or reorders enough to break that.
fn fraction_over(opened_at: u32, highest: u32) -> u8 {
    let expected = highest.saturating_sub(opened_at);
    let lost = withheld_within(opened_at, highest);
    if expected == 0 || lost == 0 {
        return 0;
    }
    u8::try_from((u64::from(lost) * 256) / u64::from(expected)).unwrap_or(u8::MAX)
}

/// How many withheld sequence numbers fall in `(opened_at, highest]`.
fn withheld_within(opened_at: u32, highest: u32) -> u32 {
    let withheld = WITHHELD
        .iter()
        .filter(|&&sequence| u32::from(sequence) > opened_at && u32::from(sequence) <= highest)
        .count();
    u32::try_from(withheld).expect("a small count")
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
