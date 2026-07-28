//! Flow keep-alives on the wire (RFC 5626 §4.4).
//!
//! Both techniques travel over the flow they are testing, which is the whole point: a ping on a
//! second connection proves a flow nobody is using. So these tests use a real socket and answer as
//! a peer would, rather than exercising the codec — the codec has its own tests against RFC 5769's
//! published vectors.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use sipx_transport::{Config, Error, Target, bind, stun};
use tokio::net::UdpSocket;

const WITHIN: Duration = Duration::from_secs(2);

/// A peer that answers STUN Binding Requests, reporting the address it saw.
async fn stun_peer(mapped: Option<&'static str>) -> std::net::SocketAddr {
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let addr = socket.local_addr().expect("has an address");
    tokio::spawn(async move {
        let mut buf = vec![0u8; 1500];
        while let Ok((len, from)) = socket.recv_from(&mut buf).await {
            let datagram = buf.get(..len).unwrap_or(&[]);
            if !stun::is_stun(datagram) {
                continue;
            }
            let id: stun::TransactionId = datagram
                .get(8..20)
                .and_then(|slice| <[u8; 12]>::try_from(slice).ok())
                .expect("a transaction id");
            let mut response = stun::binding_request(&id);
            response[0] = 0x01;
            response[1] = 0x01;
            if let Some(mapped) = mapped {
                let mapped: std::net::SocketAddr = mapped.parse().expect("valid");
                let std::net::IpAddr::V4(ip) = mapped.ip() else {
                    panic!("the fixture is IPv4");
                };
                let xor_port =
                    mapped.port() ^ u16::try_from(stun::MAGIC_COOKIE >> 16).expect("fits");
                let xor_ip = u32::from(ip) ^ stun::MAGIC_COOKIE;
                response[2] = 0x00;
                response[3] = 12;
                response.extend_from_slice(&0x0020u16.to_be_bytes());
                response.extend_from_slice(&8u16.to_be_bytes());
                response.push(0);
                response.push(0x01);
                response.extend_from_slice(&xor_port.to_be_bytes());
                response.extend_from_slice(&xor_ip.to_be_bytes());
            }
            let _ = socket.send_to(&response, from).await;
        }
    });
    addr
}

#[tokio::test]
async fn a_udp_flow_is_kept_alive_with_stun_and_learns_its_reflexive_address() {
    // §4.4.2's reason for preferring STUN to a SIP request: the answer says what address the far
    // end saw, so a UA can tell a NAT rebinding from a working flow.
    let peer = stun_peer(Some("192.0.2.1:32853")).await;
    let (endpoint, _incoming) = bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");

    let mapped = endpoint
        .keepalive(Target::udp(peer), WITHIN)
        .await
        .expect("the flow is alive");
    assert_eq!(
        mapped,
        Some("192.0.2.1:32853".parse().expect("valid")),
        "the reflexive address from XOR-MAPPED-ADDRESS"
    );
}

#[tokio::test]
async fn a_server_that_reports_no_mapped_address_still_proves_the_flow() {
    // A server is not obliged to be useful. Treating a bare Binding Response as a failure would
    // declare a working flow dead.
    let peer = stun_peer(None).await;
    let (endpoint, _incoming) = bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");

    assert_eq!(
        endpoint
            .keepalive(Target::udp(peer), WITHIN)
            .await
            .expect("the flow is alive"),
        None
    );
}

#[tokio::test]
async fn an_unanswered_keepalive_reports_the_flow_as_failed() {
    // §4.4.1: a UA whose keep-alive goes unanswered "MUST treat the flow as failed". Silence has
    // to become an error rather than a hang, or a UA waits forever on a dead flow.
    let dead = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let dead_addr = dead.local_addr().expect("has an address");
    drop(dead);

    let (endpoint, _incoming) = bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");

    let outcome = endpoint
        .keepalive(Target::udp(dead_addr), Duration::from_millis(300))
        .await;
    assert!(
        matches!(outcome, Err(Error::KeepaliveUnanswered)),
        "silence must be a failed flow, not a hang: {outcome:?}"
    );
}

#[tokio::test]
async fn a_stun_error_response_fails_the_flow_rather_than_going_unanswered() {
    // §4.4.2: "If a STUN Binding Error Response is received ... the UA considers the flow failed."
    // A refusal is a stronger signal than silence — something is there and does not want this
    // flow — so it must not be indistinguishable from a timeout.
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let peer = socket.local_addr().expect("has an address");
    tokio::spawn(async move {
        let mut buf = vec![0u8; 1500];
        if let Ok((len, from)) = socket.recv_from(&mut buf).await {
            let datagram = buf.get(..len).unwrap_or(&[]);
            let id: stun::TransactionId = datagram
                .get(8..20)
                .and_then(|slice| <[u8; 12]>::try_from(slice).ok())
                .expect("a transaction id");
            let mut error = stun::binding_request(&id);
            error[0] = 0x01;
            error[1] = 0x11;
            let _ = socket.send_to(&error, from).await;
        }
    });

    let (endpoint, _incoming) = bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");

    let outcome = endpoint.keepalive(Target::udp(peer), WITHIN).await;
    assert!(
        matches!(outcome, Err(Error::KeepaliveRefused)),
        "a refusal is not a timeout: {outcome:?}"
    );
}

#[cfg(feature = "tcp")]
#[tokio::test]
async fn a_tcp_flow_is_kept_alive_with_a_crlf_ping_and_answered_with_one_crlf() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;

    // §4.4.1: CRLFCRLF is the ping and a lone CRLF is the pong. Both are bytes RFC 3261 §7.5
    // tells a parser to ignore, which is exactly why this mechanism needs the parser to count
    // them rather than merely tolerate them.
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let peer = listener.local_addr().expect("has an address");
    let seen = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::<u8>::new()));
    let recorder = std::sync::Arc::clone(&seen);
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accepts");
        let mut buf = vec![0u8; 64];
        while let Ok(n) = stream.read(&mut buf).await {
            if n == 0 {
                break;
            }
            recorder
                .lock()
                .await
                .extend_from_slice(buf.get(..n).unwrap_or(&[]));
            // The pong: one CRLF, not two. Two would be a ping of its own.
            if stream.write_all(b"\r\n").await.is_err() {
                break;
            }
        }
    });

    let (endpoint, _incoming) = bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");

    assert_eq!(
        endpoint
            .keepalive(
                Target::new(peer, sipx_transport::TransportKind::Tcp),
                WITHIN
            )
            .await
            .expect("the flow is alive"),
        None,
        "a pong carries nothing but its own arrival"
    );
    assert_eq!(
        seen.lock().await.as_slice(),
        b"\r\n\r\n",
        "the ping is a double CRLF and nothing else"
    );
}

#[tokio::test]
async fn a_stun_reply_carrying_someone_elses_transaction_id_is_not_an_answer() {
    // RFC 5389 §6 makes the transaction ID cryptographically random, and this is what it buys: an
    // off-path attacker who cannot guess it cannot answer a keep-alive. Matching a reply to
    // whichever request happens to be outstanding would give that away — and the consequence is
    // not academic, because RFC 5626 §4.4.2 has a mapped address that *differs* from the last one
    // mean the flow has failed. A forged reply could therefore tear down a working flow.
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let peer = socket.local_addr().expect("has an address");
    tokio::spawn(async move {
        let mut buf = vec![0u8; 1500];
        if let Ok((_, from)) = socket.recv_from(&mut buf).await {
            // A well-formed Binding Response for a transaction nobody started.
            let mut forged = stun::binding_request(&[0xff; 12]);
            forged[0] = 0x01;
            forged[1] = 0x01;
            let _ = socket.send_to(&forged, from).await;
        }
    });

    let (endpoint, _incoming) = bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");

    let outcome = endpoint
        .keepalive(Target::udp(peer), Duration::from_millis(400))
        .await;
    assert!(
        matches!(outcome, Err(Error::KeepaliveUnanswered)),
        "a reply for another transaction must not answer this one: {outcome:?}"
    );
}
