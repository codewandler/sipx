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
    Call, CallEvent, CallEvents, Calls, DialOptions, Dispatched, Dispatcher, EndCause, answer,
    dial, serve,
};
use sipx_sip::{HeaderName, Host, HostName, Method, Request, Response, Uri};
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

/// A dispatcher pumped by a task of its own, with what it surfaced on a channel.
///
/// This is how a host uses one: the loop has to keep running for the ACKs and BYEs of the calls
/// it has already handed out to move, so it cannot be the same thing that answers them.
struct Pumped {
    calls: Calls,
    surfaced: Receiver<Dispatched>,
}

fn pump(endpoint: &Handle, incoming: Receiver<Incoming>, queue: usize) -> Pumped {
    let mut dispatcher = Dispatcher::with_queue(endpoint.clone(), incoming, queue);
    let calls = dispatcher.calls();
    let (tx, surfaced) = tokio::sync::mpsc::channel(8);
    tokio::spawn(async move {
        while let Some(event) = dispatcher.next().await {
            if tx.send(event).await.is_err() {
                return;
            }
        }
    });
    Pumped { calls, surfaced }
}

impl Pumped {
    async fn next(&mut self) -> Dispatched {
        tokio::time::timeout(Duration::from_secs(5), self.surfaced.recv())
            .await
            .expect("the dispatcher surfaced something")
            .expect("the dispatcher is still running")
    }

    async fn invitation(&mut self) -> (Incoming, Receiver<Incoming>) {
        match self.next().await {
            Dispatched::Invitation(invitation) => invitation.into_parts(),
            other => panic!("expected an invitation, got {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// A peer built from raw messages, so that a test can send what sipx's own side never would.
// ---------------------------------------------------------------------------------------------

fn via(endpoint: &Handle) -> bytes::Bytes {
    bytes::Bytes::from(format!(
        "SIP/2.0/UDP {};rport;branch={}",
        endpoint.sent_by_for(sipx_transport::TransportKind::Udp),
        sipx_transport::new_branch()
    ))
}

fn contact(endpoint: &Handle) -> bytes::Bytes {
    bytes::Bytes::from(format!("<sip:peer@{}>", endpoint.local_addr()))
}

fn sdp(port: u16, payload: u8, encoding: &str) -> String {
    format!(
        "v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
         m=audio {port} RTP/AVP {payload}\r\na=rtpmap:{payload} {encoding}/8000\r\na=sendrecv\r\n"
    )
}

/// A request from the raw peer. `to_tag` is `None` outside a dialog.
fn raw(
    endpoint: &Handle,
    method: &Method,
    call_id: &str,
    from_tag: &str,
    to_tag: Option<&str>,
    cseq: u32,
    body: Option<&str>,
) -> Request {
    let to = match to_tag {
        Some(tag) => format!("<sip:callee.example>;tag={tag}"),
        None => "<sip:callee.example>".to_owned(),
    };
    let mut builder = sipx_sip::build::RequestBuilder::new(method.clone(), callee_uri())
        .header(HeaderName::Via, via(endpoint))
        .expect("via")
        .header(HeaderName::To, bytes::Bytes::from(to))
        .expect("to")
        .header(
            HeaderName::From,
            bytes::Bytes::from(format!("<sip:peer@example.net>;tag={from_tag}")),
        )
        .expect("from")
        .header(HeaderName::CallId, bytes::Bytes::from(call_id.to_owned()))
        .expect("call-id")
        .cseq(cseq, method)
        .expect("cseq")
        .header(HeaderName::Contact, contact(endpoint))
        .expect("contact")
        .header(
            HeaderName::Allow,
            bytes::Bytes::from_static(b"INVITE, ACK, CANCEL, BYE, OPTIONS, UPDATE"),
        )
        .expect("allow")
        .max_forwards(70);
    if let Some(body) = body {
        builder = builder
            .header(
                HeaderName::ContentType,
                bytes::Bytes::from_static(b"application/sdp"),
            )
            .expect("content-type")
            .body(bytes::Bytes::from(body.to_owned()));
    }
    builder.build()
}

/// Send a request from the peer and read whatever final response it draws.
async fn ask(peer: &Handle, callee: SocketAddr, request: Request) -> Response {
    let mut responses = peer
        .send(request, Target::udp(callee))
        .await
        .expect("sends");
    tokio::time::timeout(Duration::from_secs(5), responses.final_response())
        .await
        .expect("the request is answered")
        .expect("a final response")
}

/// Send a request from the peer without waiting for anything.
async fn tell(peer: &Handle, callee: SocketAddr, request: Request) {
    peer.send(request, Target::udp(callee))
        .await
        .expect("sends");
}

/// The peer's side of establishing a call the callee answers with [`answer`].
///
/// Returns the answered call and the tag it chose, with the ACK already on its way through the
/// dispatcher — which is the window the reserved route exists to cover.
async fn establish(
    peer: &Handle,
    callee_endpoint: &Handle,
    pumped: &mut Pumped,
    call_id: &str,
    from_tag: &str,
) -> (Call, String, Receiver<Incoming>) {
    let callee_addr = callee_endpoint.local_addr();
    let invite = raw(
        peer,
        &Method::Invite,
        call_id,
        from_tag,
        None,
        1,
        Some(&sdp(40000, 0, "PCMU")),
    );
    let asking = {
        let peer = peer.clone();
        tokio::spawn(async move { ask(&peer, callee_addr, invite).await })
    };

    let (incoming, mut requests) = pumped.invitation().await;
    let call = answer(callee_endpoint, &incoming, loopback())
        .await
        .expect("answers");
    let accepted = asking.await.expect("the peer's INVITE is answered");
    assert_eq!(accepted.status.code(), 200, "the INVITE was not accepted");

    let tag = String::from_utf8_lossy(&call.dialog.id.local_tag).into_owned();
    tell(
        peer,
        callee_addr,
        raw(peer, &Method::Ack, call_id, from_tag, Some(&tag), 1, None),
    )
    .await;

    // Taken off the inbox here rather than left for the caller. It is the first thing the
    // reserved route carries, and a test that expects its own request next would otherwise be
    // handed the ACK — which stops the 2xx retransmitting and looks like nothing happened.
    let mut call = call;
    let ack = tokio::time::timeout(Duration::from_secs(5), requests.recv())
        .await
        .expect("the ACK is routed to the call it completes")
        .expect("the inbox is open");
    assert_eq!(ack.request.method, Method::Ack);
    assert!(call.handle(&ack).await.expect("handled"));

    (call, tag, requests)
}

/// Poll a future exactly once and then abandon it.
///
/// The one way to leave a call mid-exchange on purpose. `timeout` would do it too, but only if
/// the exchange happened to be slower than the timeout, and a test that depends on that is a
/// test that passes for a reason it does not state.
fn abandon_after_one_poll<F: Future>(future: F) {
    let waker = std::task::Waker::noop();
    let mut context = std::task::Context::from_waker(waker);
    let mut future = std::pin::pin!(future);
    assert!(
        future.as_mut().poll(&mut context).is_pending(),
        "the exchange completed instead of being abandoned part-way"
    );
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
    let dialling_a = tokio::spawn(dial_callee(a_endpoint, callee_addr, "<sip:a@example.net>"));
    let dialling_b = tokio::spawn(dial_callee(b_endpoint, callee_addr, "<sip:b@example.net>"));

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

/// Acceptance 2, the 481 half: an in-dialog request that belongs to no live call gets RFC 3261
/// §12.2.2's answer, not silence. A peer told 481 stops; a peer told nothing retransmits until
/// its own timer gives up and then believes the dialog is still there.
#[tokio::test]
async fn an_in_dialog_request_for_no_live_call_is_answered_481() {
    let (callee_endpoint, callee_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();
    let (peer_endpoint, _peer_incoming) = endpoint().await;
    let mut pumped = pump(&callee_endpoint, callee_incoming, 4);

    let refused = ask(
        &peer_endpoint,
        callee_addr,
        raw(
            &peer_endpoint,
            &Method::Bye,
            "no-such-call@sipx",
            "theirs",
            Some("ours"),
            2,
            None,
        ),
    )
    .await;

    assert_eq!(
        refused.status.code(),
        481,
        "a BYE for a dialog this endpoint does not have must be 481, not {} {}",
        refused.status.code(),
        String::from_utf8_lossy(&refused.reason)
    );
    assert_eq!(
        pumped.calls.counts().unmatched,
        1,
        "an unmatched request that is answered must also be counted"
    );
    assert!(
        pumped.surfaced.try_recv().is_err(),
        "an orphan BYE is answered by the dispatcher, not handed to the application"
    );
}

/// The same rule for a method that only exists inside a dialog and arrives without a `To` tag:
/// it is an orphan of a dialog that is gone, not an invitation to open a new exchange.
#[tokio::test]
async fn a_dialog_only_method_outside_a_dialog_is_also_481() {
    let (callee_endpoint, callee_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();
    let (peer_endpoint, _peer_incoming) = endpoint().await;
    let _pumped = pump(&callee_endpoint, callee_incoming, 4);

    let refused = ask(
        &peer_endpoint,
        callee_addr,
        raw(
            &peer_endpoint,
            &Method::Update,
            "orphan@sipx",
            "theirs",
            None,
            2,
            None,
        ),
    )
    .await;
    assert_eq!(refused.status.code(), 481);
}

/// Acceptance 2, the 405 half: RFC 3261 §8.2.1 — "the UAS MUST generate a 405 ... and MUST add
/// an Allow header field". The `Allow` is the whole value of the response, and it must be the
/// one list the rest of the stack advertises rather than a second copy of it.
#[tokio::test]
async fn an_unsupported_method_outside_a_dialog_is_refused_405() {
    let (callee_endpoint, callee_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();
    let (peer_endpoint, _peer_incoming) = endpoint().await;
    let pumped = pump(&callee_endpoint, callee_incoming, 4);

    let refused = ask(
        &peer_endpoint,
        callee_addr,
        raw(
            &peer_endpoint,
            &Method::Subscribe,
            "unsupported@sipx",
            "theirs",
            None,
            1,
            None,
        ),
    )
    .await;

    assert_eq!(
        refused.status.code(),
        405,
        "an unsupported method must be 405, not {}",
        refused.status.code()
    );
    let allow = refused
        .headers
        .value(&HeaderName::Allow)
        .expect("§8.2.1 makes Allow mandatory on a 405");
    assert_eq!(
        String::from_utf8_lossy(&allow),
        sipx_sip::update::ALLOW,
        "the 405 must advertise the one list, not a copy of it"
    );
    // RFC 3261 §8.2.6.2: every response but a 100 carries a `To` tag, and the request arrived
    // without one. A refusal a peer is entitled to discard is silence with extra steps.
    let to = refused.headers.value(&HeaderName::To).expect("a To header");
    assert!(
        String::from_utf8_lossy(&to).contains("tag="),
        "the refusal carried no To tag: {}",
        String::from_utf8_lossy(&to)
    );
    assert_eq!(pumped.calls.counts().unsupported, 1);
}

/// A method the stack *does* advertise but the dispatcher cannot place is handed to the
/// application, not refused. Refusing an OPTIONS 405 while the same message's `Allow` names
/// OPTIONS would have one endpoint tell a peer two different things.
#[tokio::test]
async fn an_advertised_method_the_dispatcher_cannot_place_is_surfaced() {
    let (callee_endpoint, callee_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();
    let (peer_endpoint, _peer_incoming) = endpoint().await;
    let mut pumped = pump(&callee_endpoint, callee_incoming, 4);

    tell(
        &peer_endpoint,
        callee_addr,
        raw(
            &peer_endpoint,
            &Method::Options,
            "ping@sipx",
            "theirs",
            None,
            1,
            None,
        ),
    )
    .await;

    match pumped.next().await {
        Dispatched::OutOfDialog(incoming) => {
            assert_eq!(incoming.request.method, Method::Options);
        }
        other => panic!("an out-of-dialog OPTIONS must reach the application, got {other:?}"),
    }
    assert_eq!(
        pumped.calls.counts().total(),
        0,
        "surfacing is not refusing, and must not be counted as loss"
    );
}

/// Acceptance 3, the vision's principle 3: one call that has stopped reading must not become
/// every other call's problem.
///
/// The stalled call's queue fills, and from then on its requests are refused `503` with a
/// `Retry-After` and counted — the deliberate shed `T-19` chose at the transport layer, applied
/// per call. Its sibling, on the same endpoint and the same dispatcher, is untouched.
#[tokio::test]
async fn a_full_call_queue_sheds_for_that_call_only() {
    let (callee_endpoint, callee_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();
    let (peer_endpoint, _peer_incoming) = endpoint().await;
    // A queue of one, so that "full" is reached in one request rather than sixteen.
    let mut pumped = pump(&callee_endpoint, callee_incoming, 1);

    // The call that stalls: its route is reserved, and nothing ever reads its inbox.
    tell(
        &peer_endpoint,
        callee_addr,
        raw(
            &peer_endpoint,
            &Method::Invite,
            "stalled@sipx",
            "stalled-tag",
            None,
            1,
            Some(&sdp(40000, 0, "PCMU")),
        ),
    )
    .await;
    let (_stalled_invite, _stalled_inbox) = pumped.invitation().await;

    // The sibling: same endpoint, same dispatcher, read normally.
    tell(
        &peer_endpoint,
        callee_addr,
        raw(
            &peer_endpoint,
            &Method::Invite,
            "healthy@sipx",
            "healthy-tag",
            None,
            1,
            Some(&sdp(40002, 0, "PCMU")),
        ),
    )
    .await;
    let (_healthy_invite, mut healthy_inbox) = pumped.invitation().await;

    // One request fills the stalled call's single slot. It is *delivered*, so there is nothing
    // to answer it and nothing to assert but that the next one is refused.
    tell(
        &peer_endpoint,
        callee_addr,
        raw(
            &peer_endpoint,
            &Method::Update,
            "stalled@sipx",
            "stalled-tag",
            Some("whatever"),
            2,
            None,
        ),
    )
    .await;

    let shed = ask(
        &peer_endpoint,
        callee_addr,
        raw(
            &peer_endpoint,
            &Method::Update,
            "stalled@sipx",
            "stalled-tag",
            Some("whatever"),
            3,
            None,
        ),
    )
    .await;
    assert_eq!(
        shed.status.code(),
        503,
        "a request for a call that is not reading must be shed with 503, not {}",
        shed.status.code()
    );
    assert!(
        shed.headers.value(&HeaderName::RetryAfter).is_some(),
        "a shed request must be told when to come back"
    );
    assert_eq!(
        pumped.calls.counts().shed,
        1,
        "a shed request that nobody counts is loss nobody is told about"
    );

    // And the sibling is exactly as it was: the dispatcher never blocked, and the stalled
    // call's backlog never touched this one's inbox.
    tell(
        &peer_endpoint,
        callee_addr,
        raw(
            &peer_endpoint,
            &Method::Update,
            "healthy@sipx",
            "healthy-tag",
            Some("whatever"),
            2,
            None,
        ),
    )
    .await;
    let delivered = tokio::time::timeout(Duration::from_secs(5), healthy_inbox.recv())
        .await
        .expect("the sibling's request arrives")
        .expect("the sibling's inbox is open");
    assert_eq!(delivered.request.method, Method::Update);
}

/// The dispatcher routes *through* RFC 3261 §12.2.2's ordering guard, not around it.
///
/// A BYE from behind the dialog's sequence number is a stale duplicate, and honouring it ends a
/// call that is still running. The guard lives on `Dialog`; this asserts the new dispatch path
/// reaches it, which is the failure a new code path beside an existing chokepoint produces.
#[tokio::test]
async fn a_replayed_bye_does_not_end_a_dispatched_call() {
    const CALL_ID: &str = "replay@sipx";
    let (callee_endpoint, callee_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();
    let (peer_endpoint, _peer_incoming) = endpoint().await;
    let mut pumped = pump(&callee_endpoint, callee_incoming, 8);

    let (mut call, tag, mut requests) = establish(
        &peer_endpoint,
        &callee_endpoint,
        &mut pumped,
        CALL_ID,
        "theirs",
    )
    .await;
    let mut events = call.events().expect("the stream has not been taken");
    assert!(
        matches!(next_event(&mut events).await, CallEvent::Answered),
        "construction queued this, and the silence asserted below is about the BYE"
    );
    tokio::spawn(async move {
        let _ = serve(&mut call, &mut requests).await;
    });

    // Move the dialog's sequence number on with a well-behaved request.
    let refreshed = ask(
        &peer_endpoint,
        callee_addr,
        raw(
            &peer_endpoint,
            &Method::Update,
            CALL_ID,
            "theirs",
            Some(&tag),
            5,
            None,
        ),
    )
    .await;
    assert_eq!(refreshed.status.code(), 200, "the refresh was refused");

    // Now the replay, from behind it.
    let replayed = ask(
        &peer_endpoint,
        callee_addr,
        raw(
            &peer_endpoint,
            &Method::Bye,
            CALL_ID,
            "theirs",
            Some(&tag),
            3,
            None,
        ),
    )
    .await;
    assert_eq!(
        replayed.status.code(),
        500,
        "a BYE from behind the sequence number must be refused, not honoured"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(200), events.recv())
            .await
            .is_err(),
        "a replayed BYE ended a live call"
    );

    // And a BYE that is genuinely next still works, so the guard refused the replay rather than
    // the method.
    let ended = ask(
        &peer_endpoint,
        callee_addr,
        raw(
            &peer_endpoint,
            &Method::Bye,
            CALL_ID,
            "theirs",
            Some(&tag),
            6,
            None,
        ),
    )
    .await;
    assert_eq!(ended.status.code(), 200);
    assert_eq!(next_ended(&mut events).await, EndCause::RemoteBye);
}

/// A request that reaches a call whose `handle` does not claim it is answered, not dropped —
/// RFC 3261 §8.2.1's 405, with the `Allow` that section makes mandatory. This is the call-layer
/// twin of the silent drop `T-19` removed at the transport layer, and `serve` used to make it.
#[tokio::test]
async fn a_method_a_call_does_not_implement_is_refused_405_by_serve() {
    const CALL_ID: &str = "unclaimed@sipx";
    let (callee_endpoint, callee_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();
    let (peer_endpoint, _peer_incoming) = endpoint().await;
    let mut pumped = pump(&callee_endpoint, callee_incoming, 8);

    let (mut call, tag, mut requests) = establish(
        &peer_endpoint,
        &callee_endpoint,
        &mut pumped,
        CALL_ID,
        "theirs",
    )
    .await;
    tokio::spawn(async move {
        let _ = serve(&mut call, &mut requests).await;
    });

    let refused = ask(
        &peer_endpoint,
        callee_addr,
        raw(
            &peer_endpoint,
            &Method::Message,
            CALL_ID,
            "theirs",
            Some(&tag),
            5,
            None,
        ),
    )
    .await;
    assert_eq!(
        refused.status.code(),
        405,
        "an in-dialog method the call does not implement must be refused, not dropped"
    );
    assert_eq!(
        refused
            .headers
            .value(&HeaderName::Allow)
            .map(|value| String::from_utf8_lossy(&value).into_owned())
            .as_deref(),
        Some(sipx_sip::update::ALLOW),
    );
}

/// And the reason that 405's `Allow` is honest: an in-dialog OPTIONS — the cheapest keep-alive
/// there is (RFC 3261 §11.2) — is answered rather than refused, because the list names it.
#[tokio::test]
async fn an_in_dialog_options_keepalive_is_answered() {
    const CALL_ID: &str = "keepalive@sipx";
    let (callee_endpoint, callee_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();
    let (peer_endpoint, _peer_incoming) = endpoint().await;
    let mut pumped = pump(&callee_endpoint, callee_incoming, 8);

    let (mut call, tag, mut requests) = establish(
        &peer_endpoint,
        &callee_endpoint,
        &mut pumped,
        CALL_ID,
        "theirs",
    )
    .await;
    tokio::spawn(async move {
        let _ = serve(&mut call, &mut requests).await;
    });

    let answered = ask(
        &peer_endpoint,
        callee_addr,
        raw(
            &peer_endpoint,
            &Method::Options,
            CALL_ID,
            "theirs",
            Some(&tag),
            5,
            None,
        ),
    )
    .await;
    assert_eq!(answered.status.code(), 200);
    assert_eq!(
        answered
            .headers
            .value(&HeaderName::Allow)
            .map(|value| String::from_utf8_lossy(&value).into_owned())
            .as_deref(),
        Some(sipx_sip::update::ALLOW),
        "an OPTIONS answered without the capability list is a wasted exchange"
    );
}

/// RFC 3311 §5.2 rule 2, end to end: an offer arriving while an offer of ours is unanswered is
/// glare, and glare is **491**, not 500 — only 491 tells the peer to back off by §14.1 rather
/// than to retry into the same wall.
///
/// `S-19` recorded this rule as having no reachable path, because a call is never mid-exchange
/// when the next request arrives. What reaches it is an *abandoned* exchange: our UPDATE was
/// left unanswered, and the peer's next one was queued in the call's inbox while it was, which
/// is exactly what a per-call inbox is for.
#[tokio::test]
async fn an_update_arriving_while_our_own_offer_is_outstanding_is_refused_491() {
    const CALL_ID: &str = "glare@sipx";
    let (callee_endpoint, callee_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();
    let (peer_endpoint, _peer_incoming) = endpoint().await;
    let mut pumped = pump(&callee_endpoint, callee_incoming, 8);

    let (mut call, tag, mut requests) = establish(
        &peer_endpoint,
        &callee_endpoint,
        &mut pumped,
        CALL_ID,
        "theirs",
    )
    .await;

    // Our own offer goes out and is never answered: the peer here does not respond to it.
    abandon_after_one_poll(call.update(sipx_sdp::Direction::SendOnly));

    let peer = peer_endpoint.clone();
    let colliding = tokio::spawn(async move {
        ask(
            &peer,
            callee_addr,
            raw(
                &peer,
                &Method::Update,
                CALL_ID,
                "theirs",
                Some(&tag),
                7,
                Some(&sdp(40004, 8, "PCMA")),
            ),
        )
        .await
    });

    let arrived = tokio::time::timeout(Duration::from_secs(5), requests.recv())
        .await
        .expect("the peer's UPDATE is routed to the call")
        .expect("the inbox is open");
    assert!(call.handle(&arrived).await.expect("handled"));

    let refused = colliding.await.expect("the peer's UPDATE is answered");
    assert_eq!(
        refused.status.code(),
        491,
        "an offer colliding with ours is glare (491), not {} {}",
        refused.status.code(),
        String::from_utf8_lossy(&refused.reason)
    );
    assert!(
        refused.headers.value(&HeaderName::RetryAfter).is_none(),
        "§5.2 gives 491 no Retry-After: glare resolves by §14.1's own back-off"
    );
}

/// RFC 3311 §5.2 rule 1, end to end: a second UPDATE arriving before the first has a final
/// response is **500 with a `Retry-After`**, and it is checked before the offer rules — the peer
/// was early, and it did not collide with anything.
#[tokio::test]
async fn an_update_arriving_while_another_is_in_progress_is_refused_500() {
    const CALL_ID: &str = "in-progress@sipx";
    let (callee_endpoint, callee_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();
    let (peer_endpoint, _peer_incoming) = endpoint().await;
    let mut pumped = pump(&callee_endpoint, callee_incoming, 8);

    let (mut call, tag, mut requests) = establish(
        &peer_endpoint,
        &callee_endpoint,
        &mut pumped,
        CALL_ID,
        "theirs",
    )
    .await;

    // Two UPDATEs, pipelined: the peer does not wait for the first to be answered. Both are off
    // the wire and in the call's inbox before either is handled, which is the arrangement a
    // per-call inbox makes ordinary.
    let first = raw(
        &peer_endpoint,
        &Method::Update,
        CALL_ID,
        "theirs",
        Some(&tag),
        7,
        Some(&sdp(40004, 8, "PCMA")),
    );
    let second = raw(
        &peer_endpoint,
        &Method::Update,
        CALL_ID,
        "theirs",
        Some(&tag),
        8,
        None,
    );
    tell(&peer_endpoint, callee_addr, first).await;
    let peer = peer_endpoint.clone();
    let asking = tokio::spawn(async move { ask(&peer, callee_addr, second).await });

    let first = tokio::time::timeout(Duration::from_secs(5), requests.recv())
        .await
        .expect("the first UPDATE is routed")
        .expect("the inbox is open");
    // Answering it is abandoned part-way, so it stays in progress.
    abandon_after_one_poll(call.handle(&first));

    let second = tokio::time::timeout(Duration::from_secs(5), requests.recv())
        .await
        .expect("the second UPDATE is routed")
        .expect("the inbox is open");
    assert!(call.handle(&second).await.expect("handled"));

    let refused = asking.await.expect("the second UPDATE is answered");
    assert_eq!(
        refused.status.code(),
        500,
        "an UPDATE arriving while one is in progress is 500, not {} {}",
        refused.status.code(),
        String::from_utf8_lossy(&refused.reason)
    );
    let retry = refused
        .headers
        .value(&HeaderName::RetryAfter)
        .expect("§5.2 requires a Retry-After on this one");
    let seconds: u64 = String::from_utf8_lossy(&retry)
        .trim()
        .parse()
        .expect("a number");
    assert!(
        seconds <= sipx_sip::update::RETRY_AFTER_MAX_SECS,
        "§5.2 asks for a value between 0 and 10 seconds, got {seconds}"
    );
}

/// A second INVITE bearing the `Call-ID` and `From` tag of one already in flight is a merged
/// request (RFC 3261 §8.2.2.2). Surfacing it as a second incoming call would hand the
/// application two calls where the peer made one, and quietly replace the first one's route.
#[tokio::test]
async fn a_merged_invite_does_not_displace_the_call_it_duplicates() {
    let (callee_endpoint, callee_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();
    let (peer_endpoint, _peer_incoming) = endpoint().await;
    let mut pumped = pump(&callee_endpoint, callee_incoming, 4);

    tell(
        &peer_endpoint,
        callee_addr,
        raw(
            &peer_endpoint,
            &Method::Invite,
            "merged@sipx",
            "theirs",
            None,
            1,
            Some(&sdp(40000, 0, "PCMU")),
        ),
    )
    .await;
    let (_invite, mut inbox) = pumped.invitation().await;

    let refused = ask(
        &peer_endpoint,
        callee_addr,
        raw(
            &peer_endpoint,
            &Method::Invite,
            "merged@sipx",
            "theirs",
            None,
            2,
            Some(&sdp(40002, 0, "PCMU")),
        ),
    )
    .await;
    assert_eq!(refused.status.code(), 482, "§8.2.2.2 names the merged case");

    // The first call's route survived it, which is the part that matters: a replacement would
    // have left the application holding an inbox nothing routes to any more.
    tell(
        &peer_endpoint,
        callee_addr,
        raw(
            &peer_endpoint,
            &Method::Bye,
            "merged@sipx",
            "theirs",
            Some("ours"),
            3,
            None,
        ),
    )
    .await;
    let delivered = tokio::time::timeout(Duration::from_secs(5), inbox.recv())
        .await
        .expect("the original route still carries")
        .expect("the inbox is open");
    assert_eq!(delivered.request.method, Method::Bye);
}

/// A call that has ended releases its route, and the dispatcher notices without being told:
/// the next request for it is answered as the unknown dialog it now is.
#[tokio::test]
async fn a_dropped_inbox_releases_its_route() {
    let (callee_endpoint, callee_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();
    let (peer_endpoint, _peer_incoming) = endpoint().await;
    let mut pumped = pump(&callee_endpoint, callee_incoming, 4);

    tell(
        &peer_endpoint,
        callee_addr,
        raw(
            &peer_endpoint,
            &Method::Invite,
            "gone@sipx",
            "theirs",
            None,
            1,
            Some(&sdp(40000, 0, "PCMU")),
        ),
    )
    .await;
    let (_invite, inbox) = pumped.invitation().await;
    assert_eq!(pumped.calls.len(), 1, "the invitation reserved a route");
    drop(inbox);

    let refused = ask(
        &peer_endpoint,
        callee_addr,
        raw(
            &peer_endpoint,
            &Method::Bye,
            "gone@sipx",
            "theirs",
            Some("ours"),
            2,
            None,
        ),
    )
    .await;
    assert_eq!(refused.status.code(), 481);
    assert!(pumped.calls.is_empty(), "the stale route was released");
}
