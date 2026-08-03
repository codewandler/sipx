//! A call whose signalling uses sipx's experimental QUIC mapping.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::IpAddr;
use std::time::Duration;

use sipx_call::{DialOptions, answer, dial};
use sipx_sip::{Host, HostName, Method, Uri};
use sipx_testkit::certs::Ca;
use sipx_transport::tls::{ClientTls, Identity, ServerTls, TrustAnchors};
use sipx_transport::{Config, Target, TransportKind, bind};

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("valid")
}

/// Q18: the dialog stays on its authenticated QUIC connection through ACK and BYE.
#[tokio::test]
async fn an_in_dialog_bye_stays_on_the_quic_connection() {
    let ca = Ca::new();
    let (cert, key) = ca.issue_for("localhost");
    let identity = Identity::from_pem(cert.as_bytes(), key.as_bytes()).expect("identity");
    let mut server_config = Config::new("127.0.0.1:0".parse().expect("address"));
    server_config.quic_server = Some((ServerTls::new(identity).expect("server"), 0));
    let (answering_endpoint, mut server_incoming) = bind(server_config).await.expect("binds");
    let quic_addr = answering_endpoint.quic_addr().expect("QUIC listener");

    let mut anchors = TrustAnchors::only();
    anchors.add_pem(ca.pem().as_bytes()).expect("CA");
    let mut client_config = Config::new("127.0.0.1:0".parse().expect("address"));
    client_config.quic_client = Some(ClientTls::new(&anchors).expect("client"));
    let (dialing_endpoint, _client_incoming) = bind(client_config).await.expect("binds");

    let answering = tokio::spawn(async move {
        let incoming = server_incoming.recv().await.expect("INVITE");
        assert_eq!(incoming.request.method, Method::Invite);
        assert_eq!(incoming.transport, TransportKind::Quic);
        let call = answer(&answering_endpoint, &incoming, loopback())
            .await
            .expect("answers");
        (call, server_incoming)
    });
    let to = Uri::sip(Host::Name(HostName::new("localhost").expect("host")));
    let target = Target::new(quic_addr, TransportKind::Quic).verifying("localhost");
    let mut outbound = tokio::time::timeout(
        Duration::from_secs(10),
        dial(
            &dialing_endpoint,
            target,
            &to,
            &DialOptions::new("<sip:caller@localhost>", loopback()),
        ),
    )
    .await
    .expect("call setup failure is bounded")
    .expect("call connects");
    let (mut inbound, mut server_incoming) = answering.await.expect("answerer");
    assert_eq!(outbound.dialog.id.call_id, inbound.dialog.id.call_id);

    outbound.hang_up().await.expect("hangs up");
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(incoming) = server_incoming.recv().await {
            assert!(inbound.handle(&incoming).await.expect("handles"));
            if inbound.is_ended() {
                return incoming.request.method;
            }
        }
        panic!("connection closed before BYE")
    })
    .await
    .expect("BYE failure is bounded");
    assert_eq!(ended, Method::Bye);
}
