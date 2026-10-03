//! Bounded sockets own capacity rather than reserving and reopening numbers.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use sipx_media::{MediaPort, MediaPortRange};
use std::net::{IpAddr, Ipv4Addr};
use tokio::net::UdpSocket;

#[test]
fn invalid_ranges_are_rejected_without_io() {
    for (first, last) in [(0, 65535), (100, 99), (101, 101), (65535, 65535)] {
        assert_eq!(
            MediaPortRange::new(first, last).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
    }
    assert!(MediaPortRange::new(101, 103).is_ok());
    assert!(MediaPortRange::new(65534, 65535).is_ok());
}

async fn held_pair(ip: IpAddr) -> MediaPort {
    // The operating system chooses the test range; no fixed port or sleep coordinates tests.
    let pair = MediaPort::bind((ip, 0).into()).await.unwrap();
    assert!(pair.has_control_port());
    assert_eq!(pair.local_addr().port() % 2, 0);
    pair
}

#[tokio::test]
async fn occupied_range_exhausts_without_fallback_then_is_reusable() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let held = held_pair(ip).await;
    let port = held.local_addr().port();
    let range = MediaPortRange::new(port, port + 1).unwrap();
    assert_eq!(
        MediaPort::bind_in_range(ip, range)
            .await
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::AddrInUse
    );
    drop(held);
    let allocated = MediaPort::bind_in_range(ip, range).await.unwrap();
    assert_eq!(allocated.local_addr().port(), port);
    assert!(allocated.has_control_port());
    assert!(UdpSocket::bind((ip, port + 1)).await.is_err());
    drop(allocated);
    assert!(MediaPort::bind_in_range(ip, range).await.is_ok());
}

#[tokio::test]
async fn occupied_control_port_does_not_leak_partial_rtp_ownership() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let held = held_pair(ip).await;
    let port = held.local_addr().port();
    drop(held);
    let control = UdpSocket::bind((ip, port + 1)).await.unwrap();
    let range = MediaPortRange::new(port, port + 1).unwrap();
    assert_eq!(
        MediaPort::bind_in_range(ip, range)
            .await
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::AddrInUse
    );
    let rtp = UdpSocket::bind((ip, port))
        .await
        .expect("failed pair released RTP");
    drop((rtp, control));
    assert!(MediaPort::bind_in_range(ip, range).await.is_ok());
}

#[tokio::test]
async fn concurrent_allocations_have_one_owner_per_pair() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let held = held_pair(ip).await;
    let port = held.local_addr().port();
    let range = MediaPortRange::new(port, port + 1).unwrap();
    drop(held);
    let (a, b) = tokio::join!(
        MediaPort::bind_in_range(ip, range),
        MediaPort::bind_in_range(ip, range)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    for result in [a, b] {
        match result {
            Ok(port) => assert!(range.contains(port.local_addr().port())),
            Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse),
        }
    }
}
