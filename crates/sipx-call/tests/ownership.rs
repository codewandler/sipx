//! Call and media worker ownership at the terminal dialog boundary (`P-15`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
#![allow(clippy::similar_names)]

use std::net::IpAddr;
use std::time::Duration;

use sipx_call::{DialOptions, answer, dial};
use sipx_sip::{Host, HostName, Method, Uri};
use sipx_transport::{Config, Target, bind};

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("valid loopback")
}

fn callee_uri() -> Uri {
    Uri::sip(Host::Name(
        HostName::new("callee.example").expect("valid host"),
    ))
}

/// An answering call owns its successful-final-response retransmitter and every worker below its
/// media session. ACK joins the former; terminal BYE handling joins the latter. Once both calls
/// are dropped, their RTP ports are therefore reusable immediately rather than after a scheduler
/// grace period.
#[tokio::test]
async fn terminal_call_operations_are_media_worker_reaping_barriers() {
    let (callee_endpoint, mut callee_incoming) =
        bind(Config::new("127.0.0.1:0".parse().expect("callee address")))
            .await
            .expect("callee binds");
    let (caller_endpoint, mut caller_incoming) =
        bind(Config::new("127.0.0.1:0".parse().expect("caller address")))
            .await
            .expect("caller binds");
    let callee_addr = callee_endpoint.local_addr();

    let answering_endpoint = callee_endpoint.clone();
    let answering = tokio::spawn(async move {
        let invite = callee_incoming.recv().await.expect("INVITE arrives");
        let call = answer(&answering_endpoint, &invite, loopback())
            .await
            .expect("call answers");
        (call, callee_incoming)
    });
    let mut caller = dial(
        &caller_endpoint,
        Target::udp(callee_addr),
        &callee_uri(),
        &DialOptions::new("<sip:caller@example.net>", loopback()),
    )
    .await
    .expect("call connects");
    let (mut callee, mut callee_incoming) = answering.await.expect("answer task joins");

    let ack = tokio::time::timeout(Duration::from_secs(2), callee_incoming.recv())
        .await // bound on failure: ACK delivery has no timing semantics.
        .expect("ACK is bounded")
        .expect("ACK arrives");
    assert_eq!(ack.request.method, Method::Ack);
    assert!(callee.handle(&ack).await.expect("ACK is handled"));

    let callee_media_addr = callee.media().local_addr();
    let caller_media_addr = caller.media().local_addr();
    let (hung_up, remote_ended) = tokio::join!(callee.hang_up(), async {
        let bye = tokio::time::timeout(Duration::from_secs(2), caller_incoming.recv())
            .await // bound on failure: BYE delivery has no timing semantics.
            .expect("BYE is bounded")
            .expect("BYE arrives");
        assert_eq!(bye.request.method, Method::Bye);
        caller.handle(&bye).await
    });
    hung_up.expect("local hangup completes");
    assert!(remote_ended.expect("remote BYE is handled"));

    drop(callee);
    drop(caller);
    let callee_rebound = tokio::net::UdpSocket::bind(callee_media_addr)
        .await
        .expect("callee media workers were reaped before hangup returned");
    let caller_rebound = tokio::net::UdpSocket::bind(caller_media_addr)
        .await
        .expect("caller media workers were reaped before BYE handling returned");
    drop(callee_rebound);
    drop(caller_rebound);

    callee_endpoint.shutdown().await;
    caller_endpoint.shutdown().await;
}
