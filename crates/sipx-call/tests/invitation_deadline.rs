//! Invitation answer and cancellation budgets from `diagnostic-phone.md` DPH-15 and DPH-16.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::IpAddr;
use std::time::Duration;

use sipx_call::{DialOptions, Error, dial};
use sipx_sip::{Host, HostName, Method, Uri};
use sipx_transport::{Config, Handle, Incoming, Target, bind};
use tokio::sync::mpsc::Receiver;

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("loopback parses")
}

async fn endpoint() -> (Handle, Receiver<Incoming>) {
    bind(Config::new("127.0.0.1:0".parse().expect("address parses")))
        .await
        .expect("endpoint binds")
}

fn target_uri() -> Uri {
    Uri::sip(Host::Name(
        HostName::new("deadline.example").expect("host parses"),
    ))
}

async fn shutdown(caller: &Handle, peer: &Handle, task: tokio::task::JoinHandle<()>) {
    caller.shutdown().await;
    peer.shutdown().await;
    task.await.expect("peer task joins");
}

/// The former implementation added the same unnamed two seconds to every answer budget. Paused
/// time makes both phases exact and proves that the tail is now caller-owned evidence.
#[tokio::test(start_paused = true)]
async fn answer_budgets_have_one_separately_measured_cancellation_allowance() {
    let cleanup_limit = Duration::from_secs(2);
    for seconds in [1, 2, 3, 5, 8] {
        let answer_limit = Duration::from_secs(seconds);
        let (peer, mut incoming) = endpoint().await;
        let (observed, mut methods) = tokio::sync::mpsc::unbounded_channel();
        let (caller, _caller_incoming) = endpoint().await;
        let options = DialOptions::new("<sip:caller@example.net>", loopback())
            .with_timeout(answer_limit)
            .with_cancellation_timeout(cleanup_limit);
        let caller_driver = caller.clone();
        let peer_address = peer.local_addr();
        let dialing = tokio::spawn(async move {
            dial(
                &caller_driver,
                Target::udp(peer_address),
                &target_uri(),
                &options,
            )
            .await
        });

        let invitation = incoming.recv().await.expect("INVITE arrives");
        assert_eq!(invitation.request.method, Method::Invite);
        let _ = observed.send(Method::Invite);
        let ringing = sipx_sip::build::ResponseBuilder::to_request(
            &invitation.request,
            sipx_sip::StatusCode::new(180).expect("status exists"),
            "Ringing",
        )
        .expect("response builds")
        .build();
        peer.respond(&invitation.key, ringing)
            .await
            .expect("ringing response sends");

        let task = tokio::spawn(async move {
            while let Some(request) = incoming.recv().await {
                let _ = observed.send(request.request.method.clone());
            }
        });
        let result = dialing.await.expect("dial task joins");
        let Error::Cancelled(cancellation) = result.expect_err("ringing peer times out") else {
            panic!("expected measured cancellation")
        };

        assert!(cancellation.timed_out);
        assert_eq!(cancellation.invitation_limit, Some(answer_limit));
        assert_eq!(cancellation.invitation_elapsed, answer_limit);
        assert_eq!(cancellation.cleanup.limit, cleanup_limit);
        assert_eq!(cancellation.cleanup.elapsed, cleanup_limit);
        assert!(cancellation.cleanup.cancel_sent());
        assert!(!cancellation.cleanup.final_response_observed());
        assert!(!cancellation.cleanup.completed());
        assert!(cancellation.cleanup.exhausted());
        let mut sent = Vec::new();
        while let Ok(method) = methods.try_recv() {
            sent.push(method);
        }
        assert!(sent.contains(&Method::Invite), "INVITE observed: {sent:?}");
        assert!(sent.contains(&Method::Cancel), "CANCEL observed: {sent:?}");
        shutdown(&caller, &peer, task).await;
    }
}

/// Zero is an explicit absence of timed cancellation waiting, not the old fixed tail and not an
/// unbounded fallback to transaction expiry.
#[tokio::test(start_paused = true)]
async fn zero_cancellation_allowance_returns_without_advancing_the_clock() {
    let silent = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("silent peer binds");
    let peer_address = silent.local_addr().expect("peer address");
    let (caller, _caller_incoming) = endpoint().await;
    let caller_driver = caller.clone();
    let options = DialOptions::new("<sip:caller@example.net>", loopback())
        .with_timeout(Duration::from_secs(1))
        .with_cancellation_timeout(Duration::ZERO);
    let dialing = tokio::spawn(async move {
        dial(
            &caller_driver,
            Target::udp(peer_address),
            &target_uri(),
            &options,
        )
        .await
    });
    let mut bytes = [0u8; 8192];
    let (length, _) = silent.recv_from(&mut bytes).await.expect("INVITE arrives");
    assert!(String::from_utf8_lossy(&bytes[..length]).starts_with("INVITE "));

    let result = dialing.await.expect("dial task joins");
    let Error::Cancelled(cancellation) = result.expect_err("silent peer times out") else {
        panic!("expected measured cancellation")
    };
    assert!(cancellation.timed_out);
    assert_eq!(cancellation.cleanup.limit, Duration::ZERO);
    assert_eq!(cancellation.cleanup.elapsed, Duration::ZERO);
    assert!(!cancellation.cleanup.cancel_sent());
    assert!(cancellation.cleanup.exhausted());
    caller.shutdown().await;
}

#[derive(Clone, Copy)]
enum FinalTiming {
    Before,
    After,
}

async fn final_response_at(timing: FinalTiming) -> Result<(), sipx_call::InvitationCancellation> {
    let answer_limit = Duration::from_secs(1);
    let (peer, mut incoming) = endpoint().await;
    let (caller, _caller_incoming) = endpoint().await;
    let options = DialOptions::new("<sip:caller@example.net>", loopback())
        .with_timeout(answer_limit)
        .with_cancellation_timeout(Duration::from_secs(2));
    let caller_driver = caller.clone();
    let peer_address = peer.local_addr();
    let dialing = tokio::spawn(async move {
        dial(
            &caller_driver,
            Target::udp(peer_address),
            &target_uri(),
            &options,
        )
        .await
    });
    let invitation = incoming.recv().await.expect("INVITE arrives");
    assert_eq!(invitation.request.method, Method::Invite);

    match timing {
        FinalTiming::Before => {}
        FinalTiming::After => {
            tokio::time::advance(answer_limit + Duration::from_millis(1)).await;
            tokio::task::yield_now().await;
        }
    }
    let response = sipx_sip::build::ResponseBuilder::to_request(
        &invitation.request,
        sipx_sip::StatusCode::new(486).expect("status exists"),
        "Busy Here",
    )
    .expect("response builds")
    .build();
    peer.respond(&invitation.key, response)
        .await
        .expect("final response sends");

    for _ in 0..100 {
        if dialing.is_finished() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(
        dialing.is_finished(),
        "final response is processed causally"
    );

    let result = dialing.await.expect("dial task joins");
    caller.shutdown().await;
    peer.shutdown().await;
    match result {
        Err(Error::Rejected { status: 486, .. }) => Ok(()),
        Err(Error::Cancelled(cancellation)) => Err(cancellation),
        other => panic!("unexpected dial result: {other:?}"),
    }
}

/// The deadline is the state transition: a final observed before it wins, while one made readable
/// after it can only complete cleanup for the already-frozen timeout. The exact-boundary ordering
/// is covered by the shared deadline primitive's paused-time unit test.
#[tokio::test(start_paused = true)]
async fn final_response_precedence_is_deterministic_at_the_answer_deadline() {
    let before = final_response_at(FinalTiming::Before).await;
    assert!(before.is_ok(), "pre-deadline final wins: {before:?}");
    let cancellation = final_response_at(FinalTiming::After)
        .await
        .expect_err("deadline freezes timeout");
    assert!(cancellation.timed_out);
    assert!(cancellation.cleanup.final_response_observed());
    assert!(cancellation.cleanup.completed());
    assert!(!cancellation.cleanup.exhausted());
}
