//! Typed transport failure propagation through outbound call setup.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::{IpAddr, Ipv4Addr, TcpListener};

use sipx_call::{DialOptions, Error, dial};
use sipx_sip::{Host, Uri};
use sipx_transport::{Config, Target, TransportKind, bind};

#[tokio::test]
async fn a_refused_stream_connection_is_not_reported_as_sip_silence() {
    let closed = TcpListener::bind("127.0.0.1:0").expect("reserves a loopback port");
    let peer = closed.local_addr().expect("reserved address");
    drop(closed);

    let (endpoint, _incoming) = bind(Config::new("127.0.0.1:0".parse().expect("local address")))
        .await
        .expect("endpoint binds");
    let to = Uri::sip(Host::Ip(peer.ip()));
    let options = DialOptions::new("<sip:caller@localhost>", IpAddr::V4(Ipv4Addr::LOCALHOST));

    let error = dial(
        &endpoint,
        Target::new(peer, TransportKind::Tcp),
        &to,
        &options,
    )
    .await
    .expect_err("the reserved port has no listener");

    assert!(matches!(error, Error::Transport(_)), "{error}");
    endpoint.shutdown().await;
}
