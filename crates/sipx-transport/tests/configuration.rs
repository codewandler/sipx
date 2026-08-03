//! Public endpoint configuration is rejected before it can create runtime resources.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::SocketAddr;
use std::time::Duration;

use sipx_transport::{Config, Error, bind};
use tokio::net::{TcpListener, UdpSocket};

async fn unused_address() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    listener.local_addr().expect("has an address")
}

async fn assert_unbound_after(error: Error, field: &'static str, address: SocketAddr) {
    assert!(
        matches!(
            error,
            Error::InvalidConfig {
                field: actual,
                reason: "must be non-zero"
            } if actual == field
        ),
        "unexpected error: {error}"
    );
    let udp = UdpSocket::bind(address)
        .await
        .expect("validation left UDP unbound");
    let tcp = TcpListener::bind(address)
        .await
        .expect("validation left TCP unbound");
    drop((udp, tcp));
}

#[tokio::test]
async fn zero_application_capacity_is_a_pre_bind_configuration_error() {
    let address = unused_address().await;
    let mut config = Config::new(address);
    config.capacity = 0;
    let error = bind(config).await.expect_err("zero capacity is invalid");
    assert_unbound_after(error, "capacity", address).await;
}

#[tokio::test]
async fn zero_pool_limit_is_a_pre_bind_configuration_error() {
    let address = unused_address().await;
    let mut config = Config::new(address);
    config.pool.max_connections = 0;
    let error = bind(config).await.expect_err("zero pool limit is invalid");
    assert_unbound_after(error, "pool.max_connections", address).await;
}

#[tokio::test]
async fn zero_handshake_budget_values_are_pre_bind_configuration_errors() {
    let address = unused_address().await;
    let mut config = Config::new(address);
    config.handshake_limit = 0;
    let error = bind(config)
        .await
        .expect_err("zero handshake limit is invalid");
    assert_unbound_after(error, "handshake_limit", address).await;

    let address = unused_address().await;
    let mut config = Config::new(address);
    config.handshake_timeout = Duration::ZERO;
    let error = bind(config)
        .await
        .expect_err("zero handshake deadline is invalid");
    assert_unbound_after(error, "handshake_timeout", address).await;
}

#[tokio::test]
async fn zero_overload_validity_is_a_pre_bind_configuration_error() {
    let address = unused_address().await;
    let mut config = Config::new(address);
    config.overload.validity = Duration::ZERO;
    let error = bind(config)
        .await
        .expect_err("zero overload validity is invalid");
    assert_unbound_after(error, "overload.validity", address).await;
}

#[tokio::test]
async fn submillisecond_overload_validity_is_rejected_before_it_rounds_to_zero_on_the_wire() {
    let address = unused_address().await;
    let mut config = Config::new(address);
    config.overload.validity = Duration::from_nanos(1);
    let error = bind(config)
        .await
        .expect_err("submillisecond overload validity cannot be represented");
    assert!(
        matches!(
            error,
            Error::InvalidConfig {
                field: "overload.validity",
                reason: "must be at least one millisecond"
            }
        ),
        "unexpected error: {error}"
    );
    let udp = UdpSocket::bind(address)
        .await
        .expect("validation left UDP unbound");
    let tcp = TcpListener::bind(address)
        .await
        .expect("validation left TCP unbound");
    drop((udp, tcp));
}

#[tokio::test]
async fn zero_overload_peer_limit_is_a_pre_bind_configuration_error() {
    let address = unused_address().await;
    let mut config = Config::new(address);
    config.overload.peer_limit = 0;
    let error = bind(config)
        .await
        .expect_err("zero overload peer-state limit is invalid");
    assert_unbound_after(error, "overload.peer_limit", address).await;
}

#[tokio::test]
async fn rate_priority_threshold_must_exceed_the_ordinary_threshold() {
    let address = unused_address().await;
    let mut config = Config::new(address);
    config.overload.rate_tolerance_intervals = 5;
    config.overload.rate_priority_tolerance_intervals = 5;
    let error = bind(config)
        .await
        .expect_err("equal rate thresholds erase priority");
    assert!(
        matches!(
            error,
            Error::InvalidConfig {
                field: "overload.rate_priority_tolerance_intervals",
                reason: "must be greater than overload.rate_tolerance_intervals"
            }
        ),
        "unexpected error: {error}"
    );
    let udp = UdpSocket::bind(address)
        .await
        .expect("validation left UDP unbound");
    let tcp = TcpListener::bind(address)
        .await
        .expect("validation left TCP unbound");
    drop((udp, tcp));
}

#[cfg(feature = "ws")]
#[tokio::test]
async fn zero_websocket_keepalive_is_a_pre_bind_configuration_error() {
    let address = unused_address().await;
    let mut config = Config::new(address);
    config.ws_keepalive = Duration::ZERO;
    let error = bind(config)
        .await
        .expect_err("zero WebSocket keepalive is invalid");
    assert_unbound_after(error, "ws_keepalive", address).await;
}

#[tokio::test]
async fn minimum_nonzero_runtime_values_bind_normally() {
    let mut config = Config::new("127.0.0.1:0".parse().expect("address"));
    config.capacity = 1;
    config.pool.max_connections = 1;
    config.handshake_limit = 1;
    config.handshake_timeout = Duration::from_nanos(1);
    #[cfg(feature = "ws")]
    {
        config.ws_keepalive = Duration::from_nanos(1);
        config.ws_server = Some(0);
    }
    #[cfg(feature = "tls")]
    {
        use sipx_testkit::certs::Ca;
        use sipx_transport::tls::{Identity, ServerTls};

        let ca = Ca::new();
        let (cert, key) = ca.issue_for("localhost");
        let identity = Identity::from_pem(cert.as_bytes(), key.as_bytes()).expect("identity");
        config.tls_server = Some((ServerTls::new(identity).expect("TLS server"), 0));
    }
    #[cfg(feature = "wss")]
    {
        use sipx_testkit::certs::Ca;
        use sipx_transport::tls::{Identity, ServerTls};

        let ca = Ca::new();
        let (cert, key) = ca.issue_for("localhost");
        let identity = Identity::from_pem(cert.as_bytes(), key.as_bytes()).expect("identity");
        config.wss_server = Some((ServerTls::new(identity).expect("WSS server"), 0));
    }

    let (endpoint, _incoming) = bind(config).await.expect("minimum values are valid");
    #[cfg(feature = "tls")]
    assert!(endpoint.tls_addr().is_some());
    #[cfg(feature = "ws")]
    assert!(endpoint.ws_addr().is_some());
    #[cfg(feature = "wss")]
    assert!(endpoint.wss_addr().is_some());
    endpoint.shutdown().await;
}
