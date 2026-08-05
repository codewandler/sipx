//! The public bounded RTP echo fixture's wire and lifecycle contract.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use std::future::{Future, poll_fn};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::num::NonZeroUsize;
use std::task::Poll;
use std::time::Duration;

use bytes::Bytes;
use sipx_audio::g711;
use sipx_rtp::Packet;
use sipx_testkit::rtp_echo::{EchoConfig, EchoError, MAX_DATAGRAM_BYTES, RtpEcho};
use sipx_testkit::soak::alive_tasks;
use tokio::net::UdpSocket;

const RUN_BOUND: Duration = Duration::from_secs(5);
const FRAME_SAMPLES: usize = 160;

fn loopback(port: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
}

fn packets(count: usize) -> NonZeroUsize {
    NonZeroUsize::new(count).expect("test packet count is non-zero")
}

fn recognizable(frame: usize) -> Vec<i16> {
    (0..FRAME_SAMPLES)
        .map(|sample| {
            let phase = i32::try_from((sample + frame * 17) % 64).unwrap_or(0) - 32;
            i16::try_from(phase * 300).unwrap_or(0)
        })
        .collect()
}

async fn assert_terminal_cleanup(echo_addr: SocketAddr, baseline_tasks: usize) {
    UdpSocket::bind(echo_addr)
        .await
        .expect("terminal error released the socket");
    assert_eq!(
        alive_tasks(),
        baseline_tasks,
        "terminal error left no owned runtime work"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_finite_stream_returns_recognizable_audio_on_one_progressing_timeline() {
    let baseline_tasks = alive_tasks();
    let peer = UdpSocket::bind(loopback(0)).await.expect("peer binds");
    let peer_addr = peer.local_addr().expect("peer address");
    let config = EchoConfig::new(loopback(0), peer_addr, packets(3), RUN_BOUND)
        .expect("bounded configuration");
    let echo = RtpEcho::bind(config).await.expect("echo binds");
    let echo_addr = echo.local_addr();

    let exchange = async {
        let mut replies = Vec::new();
        let mut headers = Vec::new();
        for frame in 0..3 {
            let source = recognizable(frame);
            let encoded = Bytes::from(g711::ulaw_encode_all(&source));
            let request = Packet::new(
                0,
                u16::MAX.wrapping_add(u16::try_from(frame).unwrap_or(0)),
                8_000 + u32::try_from(frame * 997).unwrap_or(0),
                0x1234_5678,
                encoded.clone(),
            );
            peer.send_to(&request.encode(), echo_addr)
                .await
                .expect("fixture receives input");
            let mut datagram = [0_u8; 2048];
            let (length, source_addr) = tokio::time::timeout(
                RUN_BOUND, // failure bound: a missing echo cannot hold the test indefinitely
                peer.recv_from(&mut datagram),
            )
            .await
            .expect("reply stayed inside the run bound")
            .expect("peer receives reply");
            assert_eq!(source_addr, echo_addr);
            let header: [u8; 12] = datagram[..12]
                .try_into()
                .expect("every decoded RTP reply has its fixed header");
            let packet =
                Packet::decode(&Bytes::copy_from_slice(&datagram[..length])).expect("reply is RTP");
            assert!(!packet.marker, "reply {frame} keeps the marker bit clear");
            assert!(
                packet.csrc.is_empty(),
                "reply {frame} has no contributing sources"
            );
            assert_eq!(header[0] & 0b0010_0000, 0, "reply {frame} has no padding");
            assert_eq!(header[0] & 0b0001_0000, 0, "reply {frame} has no extension");
            assert_eq!(
                header[0] & 0b0000_1111,
                0,
                "reply {frame} has no CSRC words"
            );
            assert_eq!(packet.payload_type, 0);
            assert_eq!(
                g711::ulaw_decode_all(&packet.payload),
                g711::ulaw_decode_all(&encoded),
                "reply {frame} carries the recognizable decoded samples"
            );
            headers.push(header);
            replies.push(packet);
        }
        (replies, headers)
    };

    let (report, (replies, headers)) = tokio::join!(echo.run(), exchange);
    let report = report.expect("all configured packets were echoed");
    assert_eq!(report.packets, 3);
    assert_eq!(report.samples, 3 * FRAME_SAMPLES);
    assert_eq!(replies.len(), 3);
    assert_eq!(
        headers,
        [
            [
                0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x53, 0x50, 0x58, 0x54
            ],
            [
                0x80, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0xa0, 0x53, 0x50, 0x58, 0x54
            ],
            [
                0x80, 0x00, 0x00, 0x02, 0x00, 0x00, 0x01, 0x40, 0x53, 0x50, 0x58, 0x54
            ],
        ],
        "the first twelve reply bytes are the specification vectors"
    );
    for (index, reply) in replies.iter().enumerate() {
        assert_eq!(reply.sequence, u16::try_from(index).unwrap_or(0));
        assert_eq!(
            reply.timestamp,
            u32::try_from(index * FRAME_SAMPLES).unwrap_or(0)
        );
        assert_eq!(reply.ssrc, 0x5350_5854);
    }

    UdpSocket::bind(echo_addr)
        .await
        .expect("completed fixture released its socket");
    assert_eq!(
        alive_tasks(),
        baseline_tasks,
        "the fixture created no residual runtime task"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cancelling_a_polled_run_releases_the_only_socket_without_a_task() {
    let baseline_tasks = alive_tasks();
    let peer = UdpSocket::bind(loopback(0)).await.expect("peer binds");
    let config = EchoConfig::new(
        loopback(0),
        peer.local_addr().expect("peer address"),
        packets(1),
        RUN_BOUND,
    )
    .expect("bounded configuration");
    let echo = RtpEcho::bind(config).await.expect("echo binds");
    let echo_addr = echo.local_addr();
    let mut run = Box::pin(echo.run());
    poll_fn(|context| {
        assert!(matches!(run.as_mut().poll(context), Poll::Pending));
        Poll::Ready(())
    })
    .await;

    drop(run);

    UdpSocket::bind(echo_addr)
        .await
        .expect("cancelling run released the socket");
    assert_eq!(alive_tasks(), baseline_tasks, "no detached owner exists");
}

#[tokio::test(flavor = "current_thread")]
async fn malformed_rtp_is_a_typed_terminal_error_and_releases_the_socket() {
    let baseline_tasks = alive_tasks();
    let peer = UdpSocket::bind(loopback(0)).await.expect("peer binds");
    let config = EchoConfig::new(
        loopback(0),
        peer.local_addr().expect("peer address"),
        packets(1),
        RUN_BOUND,
    )
    .expect("bounded configuration");
    let echo = RtpEcho::bind(config).await.expect("echo binds");
    let echo_addr = echo.local_addr();

    peer.send_to(b"not RTP", echo_addr)
        .await
        .expect("malformed datagram reaches fixture");
    let error = echo.run().await.expect_err("malformed RTP is refused");
    assert!(matches!(error, EchoError::Rtp(_)));
    assert_terminal_cleanup(echo_addr, baseline_tasks).await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_foreign_source_is_a_typed_terminal_error_and_releases_everything() {
    let baseline_tasks = alive_tasks();
    let peer = UdpSocket::bind(loopback(0)).await.expect("peer binds");
    let foreign = UdpSocket::bind(loopback(0))
        .await
        .expect("foreign source binds");
    let peer_addr = peer.local_addr().expect("peer address");
    let config = EchoConfig::new(loopback(0), peer_addr, packets(1), RUN_BOUND)
        .expect("bounded configuration");
    let echo = RtpEcho::bind(config).await.expect("echo binds");
    let echo_addr = echo.local_addr();
    let request = Packet::new(0, 1, 160, 0x1234_5678, Bytes::from_static(&[0xff]));

    foreign
        .send_to(&request.encode(), echo_addr)
        .await
        .expect("foreign datagram reaches fixture");
    let error = echo.run().await.expect_err("foreign source is refused");
    assert!(matches!(
        error,
        EchoError::UnexpectedPeer {
            expected,
            actual,
        } if expected == peer_addr && actual == foreign.local_addr().expect("foreign address")
    ));
    assert_terminal_cleanup(echo_addr, baseline_tasks).await;
}

#[tokio::test(flavor = "current_thread")]
async fn an_oversized_datagram_is_a_typed_terminal_error_and_releases_everything() {
    let baseline_tasks = alive_tasks();
    let peer = UdpSocket::bind(loopback(0)).await.expect("peer binds");
    let config = EchoConfig::new(
        loopback(0),
        peer.local_addr().expect("peer address"),
        packets(1),
        RUN_BOUND,
    )
    .expect("bounded configuration");
    let echo = RtpEcho::bind(config).await.expect("echo binds");
    let echo_addr = echo.local_addr();

    peer.send_to(&vec![0_u8; MAX_DATAGRAM_BYTES + 1], echo_addr)
        .await
        .expect("oversized datagram reaches fixture");
    let error = echo.run().await.expect_err("oversized datagram is refused");
    assert!(matches!(
        error,
        EchoError::DatagramTooLarge { limit } if limit == MAX_DATAGRAM_BYTES
    ));
    assert_terminal_cleanup(echo_addr, baseline_tasks).await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_non_pcmu_packet_is_a_typed_terminal_error_and_releases_everything() {
    let baseline_tasks = alive_tasks();
    let peer = UdpSocket::bind(loopback(0)).await.expect("peer binds");
    let config = EchoConfig::new(
        loopback(0),
        peer.local_addr().expect("peer address"),
        packets(1),
        RUN_BOUND,
    )
    .expect("bounded configuration");
    let echo = RtpEcho::bind(config).await.expect("echo binds");
    let echo_addr = echo.local_addr();
    let request = Packet::new(8, 1, 160, 0x1234_5678, Bytes::from_static(&[0xd5]));

    peer.send_to(&request.encode(), echo_addr)
        .await
        .expect("non-PCMU datagram reaches fixture");
    let error = echo.run().await.expect_err("non-PCMU packet is refused");
    assert!(matches!(error, EchoError::UnsupportedPayloadType(8)));
    assert_terminal_cleanup(echo_addr, baseline_tasks).await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_whole_run_deadline_reports_completed_work_and_releases_the_socket() {
    let peer = UdpSocket::bind(loopback(0)).await.expect("peer binds");
    let within = Duration::from_millis(25); // failure bound: incomplete fixture lifetime
    let config = EchoConfig::new(
        loopback(0),
        peer.local_addr().expect("peer address"),
        packets(2),
        within,
    )
    .expect("bounded configuration");
    let echo = RtpEcho::bind(config).await.expect("echo binds");
    let echo_addr = echo.local_addr();
    let payload = Bytes::from(g711::ulaw_encode_all(&recognizable(0)));
    let request = Packet::new(0, 9, 900, 0x1234_5678, payload);
    peer.send_to(&request.encode(), echo_addr)
        .await
        .expect("one of two packets reaches the fixture");

    let error = echo
        .run()
        .await
        .expect_err("the absent second packet reaches the whole-run deadline");
    assert!(matches!(
        error,
        EchoError::TimedOut {
            received: 1,
            expected: 2,
            within: elapsed,
        } if elapsed == within
    ));
    UdpSocket::bind(echo_addr)
        .await
        .expect("deadline released the socket");
}

#[test]
fn unbounded_or_unroutable_configurations_are_refused() {
    let peer = loopback(9000);
    assert!(matches!(
        EchoConfig::new(loopback(0), peer, packets(1), Duration::ZERO),
        Err(EchoError::InvalidConfig {
            field: "within",
            ..
        })
    ));
    assert!(matches!(
        EchoConfig::new(loopback(0), loopback(0), packets(1), RUN_BOUND),
        Err(EchoError::InvalidConfig { field: "peer", .. })
    ));
    assert!(matches!(
        EchoConfig::new(
            loopback(0),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 9000),
            packets(1),
            RUN_BOUND,
        ),
        Err(EchoError::InvalidConfig { field: "peer", .. })
    ));
}
