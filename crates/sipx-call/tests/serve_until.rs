//! Confirmed-call terminal-input arbitration.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::similar_names)]

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use sipx_call::{EndCause, Served, answer, dial, serve_until};
use sipx_sip::{Host, Method, Uri};
use sipx_transport::{Config, Target, bind};

#[tokio::test]
async fn a_queued_remote_bye_wins_over_a_ready_local_stop() {
    let (callee_endpoint, mut callee_incoming) = bind(Config::new(
        "127.0.0.1:0".parse().expect("callee bind address"),
    ))
    .await
    .expect("callee binds");
    let (caller_endpoint, mut caller_incoming) = bind(Config::new(
        "127.0.0.1:0".parse().expect("caller bind address"),
    ))
    .await
    .expect("caller binds");
    let address = callee_endpoint.local_addr();
    let to = Uri::sip(Host::Ip(address.ip()));
    let dialing = tokio::spawn({
        let caller_endpoint = caller_endpoint.clone();
        async move {
            dial(
                &caller_endpoint,
                Target::udp(address),
                &to,
                &sipx_call::DialOptions::new(
                    "<sip:caller@127.0.0.1>",
                    IpAddr::V4(Ipv4Addr::LOCALHOST),
                ),
            )
            .await
            .expect("call confirms")
        }
    });
    let invitation = callee_incoming.recv().await.expect("INVITE arrives");
    let mut callee = answer(
        &callee_endpoint,
        &invitation,
        IpAddr::V4(Ipv4Addr::LOCALHOST),
    )
    .await
    .expect("callee answers");
    let mut caller = dialing.await.expect("dial task joins");
    let ack = callee_incoming.recv().await.expect("ACK arrives");
    assert_eq!(ack.request.method, Method::Ack);
    assert!(callee.handle(&ack).await.expect("ACK is handled"));

    let ending = tokio::spawn(async move {
        callee
            .hang_up_observed(Duration::from_secs(2))
            .await
            .expect("remote BYE receives its response")
    });
    let remote_bye = caller_incoming.recv().await.expect("remote BYE arrives");
    assert_eq!(remote_bye.request.method, Method::Bye);
    let (queued, mut inbox) = tokio::sync::mpsc::channel(1);
    queued.send(remote_bye).await.expect("BYE is queued");
    drop(queued);

    let outcome = serve_until(
        &mut caller,
        &mut inbox,
        |_media, stopped| async move {
            stopped.cancelled().await;
            "joined"
        },
        std::future::ready(()),
    )
    .await
    .expect("call is served");
    match outcome {
        Served::Remote { cause, output } => {
            assert_eq!(cause, EndCause::RemoteBye);
            assert_eq!(output, "joined");
        }
        other => panic!("queued remote BYE lost the terminal race: {other:?}"),
    }
    assert_eq!(ending.await.expect("remote hangup task joins"), 200);
    assert!(
        callee_incoming.try_recv().is_err(),
        "the ready local stop originated a second BYE after accepting the queued remote BYE"
    );

    caller_endpoint.shutdown().await;
    callee_endpoint.shutdown().await;
}
