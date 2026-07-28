//! Blind transfer: REFER, the call it asks for, and the NOTIFYs that report the outcome.
//!
//! Three parties, which is the smallest number that makes a transfer mean anything: a
//! transferor who hands the call over, a transferee who takes it on, and a target who is called
//! as a result. Two would let a bug pass that the third catches — a transferee that reports
//! success without placing a call, most obviously.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
#![allow(clippy::similar_names)]

use std::net::IpAddr;
use std::time::Duration;

use sipx_call::transfer::TransferState;
use sipx_call::{Call, DialOptions, answer, dial};
use sipx_sip::{Host, HostName, Method, StatusCode, Uri};
use sipx_transport::{Config, Handle, Incoming, Target, bind};
use tokio::sync::mpsc::Receiver;

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("valid")
}

async fn endpoint() -> (Handle, Receiver<Incoming>) {
    bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds")
}

fn options(from: &str) -> DialOptions {
    DialOptions::new(from, loopback())
}

/// A URI naming an address directly, which is what a `Refer-To` carries when there is no proxy
/// to resolve names.
fn uri_for(addr: std::net::SocketAddr) -> Uri {
    Uri::parse(bytes::Bytes::from(format!("sip:target@{addr}"))).expect("a valid URI")
}

/// Feed everything that arrives into the call until `done` is satisfied.
///
/// Needed because the interesting request is never the first one: a callee's channel carries
/// the ACK for its own 200 before anything else, and a transferor's carries that plus every
/// NOTIFY. A test that took the first request and asserted on it would be testing the ACK.
async fn deliver_until<F>(call: &mut Call, incoming: &mut Receiver<Incoming>, done: F)
where
    F: Fn(&Call) -> bool,
{
    let pumped = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(request) = incoming.recv().await {
            assert!(
                call.handle(&request).await.expect("handles"),
                "{:?} belongs to this call",
                request.request.method
            );
            if done(call) {
                return;
            }
        }
        panic!("the channel closed before what the test was waiting for arrived");
    })
    .await;
    assert!(pumped.is_ok(), "timed out waiting on the call");
}

/// Connect a caller and a callee, and hand back both sides.
async fn connect(
    caller_endpoint: &Handle,
    callee_endpoint: Handle,
    callee_incoming: &mut Receiver<Incoming>,
    from: &str,
) -> (Call, Call) {
    let callee_addr = callee_endpoint.local_addr();
    let to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));

    let answering = async {
        let incoming = callee_incoming.recv().await.expect("an INVITE arrives");
        answer(&callee_endpoint, &incoming, loopback())
            .await
            .expect("answers")
    };
    let dial_options = options(from);
    let dialling = dial(
        caller_endpoint,
        Target::udp(callee_addr),
        &to,
        &dial_options,
    );

    let (callee, caller) = tokio::join!(answering, dialling);
    (caller.expect("the call connects"), callee)
}

/// S-9's exit criterion: the transfer reaches the target, and the transferor is told.
#[tokio::test]
async fn a_referred_call_reaches_the_target_and_notifies_the_transferor() {
    let (alice_endpoint, mut alice_incoming) = endpoint().await;
    let (bob_endpoint, mut bob_incoming) = endpoint().await;
    let (carol_endpoint, mut carol_incoming) = endpoint().await;
    let carol_addr = carol_endpoint.local_addr();

    // Alice calls Bob.
    let (mut alice, mut bob) = connect(
        &alice_endpoint,
        bob_endpoint.clone(),
        &mut bob_incoming,
        "<sip:alice@example.net>",
    )
    .await;

    // Carol will answer whatever arrives.
    let carol = tokio::spawn(async move {
        let incoming = carol_incoming.recv().await.expect("an INVITE from Bob");
        assert_eq!(incoming.request.method, Method::Invite);
        let call = answer(&carol_endpoint, &incoming, loopback())
            .await
            .expect("Carol answers");
        (call, incoming.request)
    });

    // Bob takes the REFER on and follows it. He has to be running while Alice refers: the 202
    // is his to send, and Alice is waiting for it.
    let bobs_side = tokio::spawn(async move {
        deliver_until(&mut bob, &mut bob_incoming, |c| c.referral().is_some()).await;

        let referral = bob.referral().expect("a referral was recorded");
        assert!(
            referral.referred_by.is_some(),
            "RFC 3892: the transferee is entitled to know who asked"
        );
        let target = Target::udp(carol_addr);
        let onward = bob
            .accept_referral(target, &options("<sip:bob@example.net>"))
            .await
            .expect("Bob places the transferred call");
        (bob, onward)
    });

    alice
        .refer(&uri_for(carol_addr))
        .await
        .expect("Bob accepts the request");

    // The 202 says only "I will try". What happened arrives as NOTIFY.
    assert_eq!(
        alice.transfer().expect("a transfer is in flight").state,
        TransferState::Trying,
        "a 202 is not success"
    );

    deliver_until(&mut alice, &mut alice_incoming, |c| {
        c.transfer().is_some_and(|t| t.finished)
    })
    .await;

    let transfer = alice.transfer().expect("a transfer");
    assert_eq!(transfer.state, TransferState::Succeeded);
    assert!(
        transfer.finished,
        "the implicit subscription must not be left running"
    );

    let (_bob, _onward) = bobs_side.await.expect("Bob finishes");
    let (_carol, invite) = carol.await.expect("Carol finishes");

    // The call really went where the REFER said, and not merely somewhere.
    let request_uri = String::from_utf8_lossy(&invite.uri.to_bytes()).into_owned();
    assert!(
        request_uri.contains(&carol_addr.to_string()),
        "the transferred call must be addressed to the Refer-To target: {request_uri}"
    );
}

/// A transfer to somewhere that refuses. The transferor must be told *that*, not left to infer
/// it from silence — this is the case that separates a real implementation of RFC 3515 §2.4.4
/// from one that reports the 202 and stops.
#[tokio::test]
async fn a_transfer_the_target_refuses_is_reported_as_a_failure() {
    let (alice_endpoint, mut alice_incoming) = endpoint().await;
    let (bob_endpoint, mut bob_incoming) = endpoint().await;
    let (carol_endpoint, mut carol_incoming) = endpoint().await;
    let carol_addr = carol_endpoint.local_addr();

    let (mut alice, mut bob) = connect(
        &alice_endpoint,
        bob_endpoint.clone(),
        &mut bob_incoming,
        "<sip:alice@example.net>",
    )
    .await;

    // Carol is busy.
    tokio::spawn(async move {
        let incoming = carol_incoming.recv().await.expect("an INVITE");
        let busy = sipx_sip::build::ResponseBuilder::to_request(
            &incoming.request,
            StatusCode::new(486).expect("valid"),
            "Busy Here",
        )
        .expect("builds")
        .build();
        carol_endpoint
            .respond(&incoming.key, busy)
            .await
            .expect("refuses");
    });

    let bobs_side = tokio::spawn(async move {
        deliver_until(&mut bob, &mut bob_incoming, |c| c.referral().is_some()).await;
        let outcome = bob
            .accept_referral(Target::udp(carol_addr), &options("<sip:bob@example.net>"))
            .await;
        assert!(outcome.is_err(), "Carol refused");
        bob
    });

    alice.refer(&uri_for(carol_addr)).await.expect("accepted");

    deliver_until(&mut alice, &mut alice_incoming, |c| {
        c.transfer().is_some_and(|t| t.finished)
    })
    .await;

    let transfer = alice.transfer().expect("a transfer");
    assert_eq!(
        transfer.state,
        TransferState::Failed {
            status: 486,
            reason: "Busy Here".to_owned()
        },
        "the transferor must learn what the target said, not merely that it did not work"
    );
    assert!(transfer.finished);

    let _bob = bobs_side.await.expect("Bob finishes");
}

/// A REFER the transferee will not honour is refused with a status the transferor can act on —
/// and refusing creates no subscription, so nothing further is owed (RFC 3515 §2.4.2).
#[tokio::test]
async fn a_refer_that_cannot_be_honoured_is_rejected() {
    let (alice_endpoint, mut alice_incoming) = endpoint().await;
    let (bob_endpoint, mut bob_incoming) = endpoint().await;
    let elsewhere = "127.0.0.1:9".parse().expect("valid");

    let (mut alice, mut bob) = connect(
        &alice_endpoint,
        bob_endpoint.clone(),
        &mut bob_incoming,
        "<sip:alice@example.net>",
    )
    .await;

    let bobs_side = tokio::spawn(async move {
        deliver_until(&mut bob, &mut bob_incoming, |c| c.referral().is_some()).await;
        bob.refuse_referral(603, "Decline").await.expect("refuses");
        bob
    });

    let error = alice
        .refer(&uri_for(elsewhere))
        .await
        .expect_err("Bob declined");
    match error {
        sipx_call::Error::Rejected { status, .. } => assert_eq!(status, 603),
        other => panic!("expected a rejection Alice can act on, got {other}"),
    }

    // And no subscription was created, so no NOTIFY follows.
    let quiet = tokio::time::timeout(Duration::from_millis(400), alice_incoming.recv()).await;
    assert!(
        quiet.is_err(),
        "a refused REFER creates no subscription and owes no notification"
    );
    assert!(alice.transfer().is_none());

    let _bob = bobs_side.await.expect("Bob finishes");
}

/// A `Refer-To` that names nothing usable is not an application decision. There is nowhere to
/// transfer to, and 400 says so without waiting for anyone to think about it.
#[tokio::test]
async fn a_refer_with_an_unusable_refer_to_is_rejected_without_asking() {
    use bytes::Bytes;
    use sipx_sip::HeaderName;
    use sipx_sip::build::RequestBuilder;

    let (alice_endpoint, _alice_incoming) = endpoint().await;
    let (bob_endpoint, mut bob_incoming) = endpoint().await;

    let (alice, mut bob) = connect(
        &alice_endpoint,
        bob_endpoint.clone(),
        &mut bob_incoming,
        "<sip:alice@example.net>",
    )
    .await;

    // A REFER built by hand, because the library will not build a broken one.
    let (local, remote) = alice.dialog.local_and_remote();
    let refer = RequestBuilder::new(Method::Refer, alice.dialog.remote_target.clone())
        .header(HeaderName::To, Bytes::from(remote))
        .expect("valid")
        .header(HeaderName::From, Bytes::from(local))
        .expect("valid")
        .header(
            HeaderName::CallId,
            Bytes::from(alice.dialog.id.call_id.clone()),
        )
        .expect("valid")
        .cseq(99, &Method::Refer)
        .expect("valid")
        .header(HeaderName::ReferTo, Bytes::from_static(b"<not a uri>"))
        .expect("valid")
        .max_forwards(70)
        .build();

    let bobs_side = tokio::spawn(async move {
        // Everything that arrives, for as long as anything does. Nothing here should ever leave
        // a referral pending: a `Refer-To` naming nothing usable is answered outright.
        let _ = tokio::time::timeout(Duration::from_secs(3), async {
            while let Some(incoming) = bob_incoming.recv().await {
                let _ = bob.handle(&incoming).await;
                assert!(
                    bob.referral().is_none(),
                    "nothing usable was asked for, so nothing is pending an answer"
                );
            }
        })
        .await;
        bob
    });

    let mut responses = alice_endpoint
        .send(refer, Target::udp(bob_endpoint.local_addr()))
        .await
        .expect("sends");
    let response = tokio::time::timeout(Duration::from_secs(5), responses.final_response())
        .await
        .expect("no timeout")
        .expect("a final response");
    assert_eq!(response.status.code(), 400);

    let _bob = bobs_side.await.expect("Bob finishes");
}
