//! Many calls on one endpoint (story `C-4`).
//!
//! `two_calls_served_concurrently_from_one_endpoint` is the failing-first test the story names.
//! Before it there was no way to hold two calls on one endpoint without hand-rolling the
//! demultiplexer, and every hand-rolled copy is a fresh chance to drop an ACK.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
#![allow(clippy::similar_names)]

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use sipx_call::{
    Call, CallEvent, CallEvents, DialOptions, Dispatched, Dispatcher, EndCause, answer, dial, serve,
};
use sipx_sip::{Host, HostName, Uri};
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

fn callee_uri() -> Uri {
    Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")))
}

/// Dial the one callee from an endpoint of this caller's own.
async fn dial_callee(caller: Handle, callee: SocketAddr, from: &str) -> Call {
    dial(
        &caller,
        Target::udp(callee),
        &callee_uri(),
        &DialOptions::new(from, loopback()),
    )
    .await
    .expect("the call connects")
}

/// The next event, bounded so a test that is wrong about wiring fails instead of hanging.
async fn next_event(events: &mut CallEvents) -> CallEvent {
    tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("no timeout waiting for a call event")
        .expect("the stream ended before this event arrived")
}

/// The next `Ended`, skipping whatever construction queued ahead of it.
async fn next_ended(events: &mut CallEvents) -> EndCause {
    loop {
        if let CallEvent::Ended(cause) = next_event(events).await {
            return cause;
        }
    }
}

/// One call the callee is serving: which dialog it is, and what it reports.
struct Served {
    call_id: Vec<u8>,
    events: CallEvents,
}

/// The story's failing-first test.
///
/// Two calls, one endpoint, one dispatcher. Both are up at the same time, each is served by its
/// own task off its own bounded inbox, and hanging one up ends *that* one — the sibling is
/// still there to be hung up afterwards.
#[tokio::test]
async fn two_calls_served_concurrently_from_one_endpoint() {
    let (callee_endpoint, callee_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    // One dispatcher over the one endpoint, pumped by a task of its own — which is what keeps
    // the ACKs and BYEs of every call it has already handed out moving while the application
    // is busy answering the next invitation.
    let mut dispatcher = Dispatcher::new(callee_endpoint.clone(), callee_incoming);
    let (invitations, mut arriving) = tokio::sync::mpsc::channel(4);
    let pump = tokio::spawn(async move {
        while let Some(event) = dispatcher.next().await {
            if let Dispatched::Invitation(invitation) = event
                && invitations.send(invitation).await.is_err()
            {
                return;
            }
        }
    });

    let (a_endpoint, _a_incoming) = endpoint().await;
    let (b_endpoint, _b_incoming) = endpoint().await;
    let dialling_a = tokio::spawn(dial_callee(
        a_endpoint,
        callee_addr,
        "<sip:a@example.net>".into(),
    ));
    let dialling_b = tokio::spawn(dial_callee(
        b_endpoint,
        callee_addr,
        "<sip:b@example.net>".into(),
    ));

    let mut served = Vec::new();
    for _ in 0..2 {
        let invitation = tokio::time::timeout(Duration::from_secs(5), arriving.recv())
            .await
            .expect("an invitation arrives")
            .expect("the dispatcher is still running");
        let (invite, mut requests) = invitation.into_parts();
        let mut call = answer(&callee_endpoint, &invite, loopback())
            .await
            .expect("answers");
        let call_id = call.dialog.id.call_id.clone();
        let events = call.events().expect("the stream has not been taken");
        // Each call is driven off its own inbox, in its own task: the whole point of the
        // dispatcher is that one of these being slow is not the others' problem.
        tokio::spawn(async move {
            let _ = serve(&mut call, &mut requests).await;
        });
        served.push(Served { call_id, events });
    }

    // Both are answered, so both have said so. Drained here rather than skipped later, so that
    // the "the sibling heard nothing" assertion below is about the BYE and not about whatever
    // construction had already queued.
    for served in &mut served {
        assert!(
            matches!(next_event(&mut served.events).await, CallEvent::Answered),
            "every call reports being answered"
        );
    }

    let mut caller_a = dialling_a.await.expect("the dialling task finishes");
    let mut caller_b = dialling_b.await.expect("the dialling task finishes");
    assert_ne!(
        caller_a.dialog.id.call_id, caller_b.dialog.id.call_id,
        "two calls, not one"
    );

    // Both calls are up on the one endpoint at the same moment. Sorted so that the assertions
    // below name a side rather than an arrival order.
    let position = |call_id: &[u8]| {
        served
            .iter()
            .position(|s| s.call_id == call_id)
            .expect("the callee is serving this call")
    };
    let a = position(&caller_a.dialog.id.call_id);
    let b = position(&caller_b.dialog.id.call_id);
    assert_ne!(a, b, "the two dialogs were routed to the same call");

    caller_a.hang_up().await.expect("hangs up");
    assert_eq!(
        next_ended(&mut served[a].events).await,
        EndCause::RemoteBye,
        "the BYE reached the call it belonged to"
    );

    // And only that one. The sibling has not been told anything: a dispatcher that routed the
    // BYE to both, or that stopped pumping when one call ended, would fail here.
    assert!(
        tokio::time::timeout(Duration::from_millis(200), served[b].events.recv())
            .await
            .is_err(),
        "the other call was ended by its sibling's BYE"
    );

    caller_b.hang_up().await.expect("hangs up");
    assert_eq!(
        next_ended(&mut served[b].events).await,
        EndCause::RemoteBye,
        "the second call is still reachable after the first one ended"
    );

    drop(arriving);
    let _ = tokio::time::timeout(Duration::from_secs(2), pump).await;
}
