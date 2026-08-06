//! The UAS half of CANCEL (story `S-23`, RFC 3261 §9.2).
//!
//! `a_caller_that_gives_up_before_the_answer_ends_the_invitation` is the failing-first test the
//! story names. Before it, a CANCEL reached the invitation's inbox and stopped there: no `200`
//! for the CANCEL, no `487` for the INVITE it withdraws, and no way for an application holding
//! an [`Invitation`](sipx_call::Invitation) to learn the caller had gone.
//!
//! The vectors are `docs/specs/call-dispatch.md` §9.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use sipx_call::{DialOptions, Dispatched, Dispatcher, Error, Invitation, answer, ring, serve};
use sipx_sip::{HeaderName, Host, HostName, Method, Request, Response, Uri};
use sipx_transport::{Config, Handle, Incoming, Target, bind};
use tokio::sync::Notify;
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

fn sdp() -> String {
    "v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
     m=audio 40000 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\na=sendrecv\r\n"
        .to_owned()
}

/// A `Via` of this peer's own, with a branch a test can hold on to.
///
/// Built here rather than left to [`Handle::send`] because RFC 3261 §9.1 makes the CANCEL carry
/// the *same* branch as the INVITE it withdraws, and that identity is the whole matching rule.
fn via(endpoint: &Handle, branch: &str) -> Bytes {
    Bytes::from(format!(
        "SIP/2.0/UDP {};rport;branch={branch}",
        endpoint.sent_by_for(sipx_transport::TransportKind::Udp)
    ))
}

/// An INVITE from a raw peer, on a branch the caller names.
fn invite(peer: &Handle, call_id: &str, from_tag: &str, branch: &str) -> Request {
    invite_with_body(peer, call_id, from_tag, branch, sdp())
}

/// The same, with an offer the caller chooses — including one that is not SDP at all.
fn invite_with_body(
    peer: &Handle,
    call_id: &str,
    from_tag: &str,
    branch: &str,
    body: String,
) -> Request {
    sipx_sip::build::RequestBuilder::new(Method::Invite, callee_uri())
        .header(HeaderName::Via, via(peer, branch))
        .expect("via")
        .header(HeaderName::To, Bytes::from_static(b"<sip:callee.example>"))
        .expect("to")
        .header(
            HeaderName::From,
            Bytes::from(format!("<sip:peer@example.net>;tag={from_tag}")),
        )
        .expect("from")
        .header(HeaderName::CallId, Bytes::from(call_id.to_owned()))
        .expect("call-id")
        .cseq(1, &Method::Invite)
        .expect("cseq")
        .header(
            HeaderName::Contact,
            Bytes::from(format!("<sip:peer@{}>", peer.local_addr())),
        )
        .expect("contact")
        .header(
            HeaderName::Allow,
            Bytes::from_static(b"INVITE, ACK, CANCEL, BYE, OPTIONS, UPDATE"),
        )
        .expect("allow")
        .max_forwards(70)
        .header(
            HeaderName::ContentType,
            Bytes::from_static(b"application/sdp"),
        )
        .expect("content-type")
        .body(Bytes::from(body))
        .build()
}

/// The CANCEL for an INVITE, built the way RFC 3261 §9.1 requires.
///
/// Same Request-URI, `Call-ID`, `To`, `From` and `CSeq` number; the method changed; and the
/// topmost `Via` copied verbatim so that the branch names the transaction being withdrawn.
fn cancel_for(request: &Request) -> Request {
    cancel_with(request, |value| value)
}

/// The same, with the topmost `Via` rewritten — for a CANCEL that names the wrong transaction.
fn cancel_with(request: &Request, rewrite: impl FnOnce(String) -> String) -> Request {
    let copy = |name: &HeaderName| {
        request
            .headers
            .value(name)
            .map(|value| Bytes::from(value.into_owned()))
    };
    let top_via =
        rewrite(String::from_utf8_lossy(&copy(&HeaderName::Via).expect("via")).into_owned());
    let sequence = request
        .headers
        .typed::<sipx_sip::headers::CSeq>()
        .and_then(std::result::Result::ok)
        .map_or(1, |cseq| cseq.sequence);

    let mut builder = sipx_sip::build::RequestBuilder::new(Method::Cancel, request.uri.clone())
        .header(HeaderName::Via, Bytes::from(top_via))
        .expect("via");
    for name in [HeaderName::To, HeaderName::From, HeaderName::CallId] {
        if let Some(value) = copy(&name) {
            builder = builder.header(name, value).expect("header");
        }
    }
    builder
        .cseq(sequence, &Method::Cancel)
        .expect("cseq")
        .max_forwards(70)
        .build()
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

/// A dispatcher pumped by a task of its own, with what it surfaced on a channel.
struct Pumped {
    surfaced: Receiver<Dispatched>,
}

fn pump(endpoint: &Handle, incoming: Receiver<Incoming>) -> Pumped {
    let mut dispatcher = Dispatcher::new(endpoint.clone(), incoming);
    let (tx, surfaced) = tokio::sync::mpsc::channel(8);
    tokio::spawn(async move {
        while let Some(event) = dispatcher.next().await {
            if tx.send(event).await.is_err() {
                return;
            }
        }
    });
    Pumped { surfaced }
}

impl Pumped {
    async fn next(&mut self) -> Dispatched {
        tokio::time::timeout(Duration::from_secs(5), self.surfaced.recv())
            .await
            .expect("the dispatcher surfaced something")
            .expect("the dispatcher is still running")
    }

    async fn invitation(&mut self) -> Invitation {
        match self.next().await {
            Dispatched::Invitation(invitation) => invitation,
            other => panic!("expected an invitation, got {other:?}"),
        }
    }
}

/// The `To` tag of a response, for the §9.2 rule that the two responses agree on one.
fn to_tag(response: &Response) -> Option<String> {
    let value = response.headers.value(&HeaderName::To)?;
    let address = sipx_sip::Address::parse(&value, "To").ok()?;
    address
        .tag()
        .map(|tag| String::from_utf8_lossy(tag).into_owned())
}

/// The story's failing-first test.
///
/// A caller rings, gives up before the callee answers, and sends the CANCEL RFC 3261 §9.1
/// describes. §9.2 then owes **two** responses on **two** transactions — `200 OK` for the
/// CANCEL and `487 Request Terminated` for the INVITE it withdraws — and the application
/// holding the invitation has to learn that it is over.
#[tokio::test]
async fn a_caller_that_gives_up_before_the_answer_ends_the_invitation() {
    let (callee, callee_incoming) = endpoint().await;
    let callee_addr = callee.local_addr();
    let mut pumped = pump(&callee, callee_incoming);
    let (peer, _peer_incoming) = endpoint().await;

    let invite = invite(&peer, "give-up@sipx", "caller", "z9hG4bK-s23-invite");
    let mut invited = peer
        .send(invite.clone(), Target::udp(callee_addr))
        .await
        .expect("the INVITE goes out");

    // The application is handed the invitation and starts ringing. It has not answered, which
    // is the whole situation §9.2 is about.
    let invitation = pumped.invitation().await;
    let _ringing = ring(&callee, invitation.request(), 180, "Ringing", false)
        .await
        .expect("rings");
    assert!(!invitation.is_cancelled(), "nothing has been cancelled yet");

    // The caller gives up.
    let cancelled = ask(&peer, callee_addr, cancel_for(&invite)).await;
    assert_eq!(
        cancelled.status.code(),
        200,
        "§9.2: a CANCEL that matches a transaction is answered 200 OK"
    );

    // The other half, on the other transaction. This is the one that was missing entirely.
    let terminated = tokio::time::timeout(Duration::from_secs(5), invited.final_response())
        .await
        .expect("the INVITE is answered")
        .expect("a final response to the INVITE");
    assert_eq!(
        terminated.status.code(),
        487,
        "§9.2: the INVITE a CANCEL withdraws is answered 487 Request Terminated"
    );

    // §9.2: "the To tag of the response to the CANCEL and the To tag in the response to the
    // original request SHOULD be the same".
    assert_eq!(
        to_tag(&cancelled),
        to_tag(&terminated),
        "both responses must carry the same To tag"
    );

    // And the application learns, because a host holding a ringing call has to stop ringing.
    assert!(
        invitation.is_cancelled(),
        "the application must be able to see that the caller gave up"
    );
    assert!(
        matches!(
            invitation.answer(&callee, loopback()).await,
            Err(sipx_call::Error::InvitationCancelled)
        ),
        "an invitation that was cancelled must not be answerable afterwards"
    );
}

/// M-69: an unparseable initial offer is itself answered 400 and claims the invitation immediately
/// before that response leaves. A later CANCEL still receives 200 for its own transaction, but it
/// cannot replace the already-sent 400 with 487. Both responses retain the invitation's one tag.
#[tokio::test]
async fn a_malformed_invitation_is_refused_before_a_late_cancel() {
    let (callee, callee_incoming) = endpoint().await;
    let callee_addr = callee.local_addr();
    let mut pumped = pump(&callee, callee_incoming);
    let (peer, _peer_incoming) = endpoint().await;

    // An INVITE whose body is not SDP at all. Everything about the transaction is well formed.
    let invite = invite_with_body(
        &peer,
        "answer-failed@sipx",
        "caller",
        "z9hG4bK-s23-failed",
        "this is not an SDP body".to_owned(),
    );
    let mut invited = peer
        .send(invite.clone(), Target::udp(callee_addr))
        .await
        .expect("the INVITE goes out");

    let invitation = pumped.invitation().await;
    assert!(
        matches!(
            invitation.answer(&callee, loopback()).await,
            Err(sipx_call::Error::Sdp(_))
        ),
        "an offer that cannot be parsed fails the answer"
    );

    let refused = tokio::time::timeout(Duration::from_secs(5), invited.final_response())
        .await
        .expect("the malformed invitation is answered")
        .expect("a final response to the INVITE");
    assert_eq!(refused.status.code(), 400);

    assert!(
        !invitation.is_cancelled(),
        "a local 400 refusal is an answer, not a caller cancellation"
    );

    // The CANCEL transaction is still answered, but it arrived after the INVITE's final response.
    let cancelled = ask(&peer, callee_addr, cancel_for(&invite)).await;
    assert_eq!(cancelled.status.code(), 200);
    assert_eq!(
        to_tag(&cancelled),
        to_tag(&refused),
        "the late CANCEL and malformed-offer response share the invitation tag"
    );
    assert!(!invitation.is_cancelled());
}

/// `Invitation::answer`'s future is `Send`, so it can still be spawned.
///
/// A compile-time assertion rather than a behaviour: answering carries a `call::Claim`, which is
/// a reference to a trait object, and `&T` is `Send` only while `T: Sync`. Dropping either bound
/// from that alias would make this future unspawnable — a break in a public API that no
/// behavioural test here would notice, because `#[tokio::test]` drives them on one thread.
#[test]
fn an_answer_future_is_spawnable() {
    fn assert_send<T: Send>(_: T) {}

    // Never polled — constructing the future is what type-checks the bound — but referenced
    // below so that the chain is live code and the assertion is really compiled.
    #[expect(unreachable_code, unused_variables, clippy::diverging_sub_expression)]
    fn witness() {
        let invitation: Invitation = unreachable!();
        let endpoint: Handle = unreachable!();
        assert_send(invitation.answer(&endpoint, loopback()));
    }

    let _ = witness as fn();
}

/// §9.2: "If the UAS did not find a matching transaction for the CANCEL ... it SHOULD respond
/// with a 481". Not dropped, and not the 405 an unplaced method would otherwise draw.
#[tokio::test]
async fn a_cancel_for_no_invitation_of_ours_is_answered_481() {
    let (callee, callee_incoming) = endpoint().await;
    let callee_addr = callee.local_addr();
    let mut _pumped = pump(&callee, callee_incoming);
    let (peer, _peer_incoming) = endpoint().await;

    let stranger = invite(&peer, "never-sent@sipx", "caller", "z9hG4bK-s23-stranger");
    let answered = ask(&peer, callee_addr, cancel_for(&stranger)).await;
    assert_eq!(
        answered.status.code(),
        481,
        "a CANCEL matching no transaction is refused 481, not dropped"
    );
}

/// The matching is §9.2's: the topmost `Via` branch plus the method of the transaction being
/// cancelled. A CANCEL that shares the `Call-ID`, `From` tag and `CSeq` of a live invitation
/// but names a different branch names a different transaction — one we do not have.
#[tokio::test]
async fn a_cancel_on_another_branch_does_not_match_by_call_id_alone() {
    let (callee, callee_incoming) = endpoint().await;
    let callee_addr = callee.local_addr();
    let mut pumped = pump(&callee, callee_incoming);
    let (peer, _peer_incoming) = endpoint().await;

    let invite = invite(&peer, "branchy@sipx", "caller", "z9hG4bK-s23-right");
    let mut invited = peer
        .send(invite.clone(), Target::udp(callee_addr))
        .await
        .expect("the INVITE goes out");
    let invitation = pumped.invitation().await;

    let elsewhere = cancel_with(&invite, |via| {
        via.replace("z9hG4bK-s23-right", "z9hG4bK-s23-wrong")
    });
    let answered = ask(&peer, callee_addr, elsewhere).await;
    assert_eq!(
        answered.status.code(),
        481,
        "the Call-ID and CSeq are not the match; the branch is"
    );
    assert!(
        !invitation.is_cancelled(),
        "an invitation must not be ended by a CANCEL that names another transaction"
    );

    // And the invitation really is still live: it can still be answered.
    let call = invitation.answer(&callee, loopback()).await;
    assert!(
        call.is_ok(),
        "the invitation survived the mismatched CANCEL"
    );
    let accepted = tokio::time::timeout(Duration::from_secs(5), invited.final_response())
        .await
        .expect("the INVITE is answered")
        .expect("a final response");
    assert_eq!(accepted.status.code(), 200);
}

/// §9.2 is explicit that a CANCEL has no effect on a transaction that has already sent a final
/// response, and BYE — not CANCEL — is what ends an established dialog. Tested as a negative:
/// the call is still up afterwards.
#[tokio::test]
async fn a_cancel_after_the_answer_does_not_tear_the_dialog_down() {
    let (callee, callee_incoming) = endpoint().await;
    let callee_addr = callee.local_addr();
    let mut pumped = pump(&callee, callee_incoming);
    let (peer, _peer_incoming) = endpoint().await;

    let invite = invite(&peer, "too-late@sipx", "caller", "z9hG4bK-s23-late");
    let asking = {
        let peer = peer.clone();
        let invite = invite.clone();
        tokio::spawn(async move { ask(&peer, callee_addr, invite).await })
    };

    let mut invitation = pumped.invitation().await;
    // Taken before the answer, because it is the sharpest instrument this test has: the
    // invitation's stream carries exactly one event, and it is the one a CANCEL that took effect
    // would produce. Asserting the *absence* of it is what makes this a negative rather than a
    // description — the `serve` loop below survives a stray 487 either way, so on its own it
    // would pass against an implementation that let a late CANCEL through.
    let mut events = invitation
        .events()
        .expect("an invitation has one event stream");

    let mut call = invitation
        .answer(&callee, loopback())
        .await
        .expect("answers");
    let accepted = asking.await.expect("answered");
    assert_eq!(accepted.status.code(), 200);

    let (_invite, mut requests) = invitation.into_parts();
    let served = tokio::spawn(async move {
        let _ = serve(&mut call, &mut requests).await;
        call
    });

    // The caller changes its mind too late. §9.2: the transaction has already answered, so the
    // CANCEL is answered 200 and does nothing else.
    let answered = ask(&peer, callee_addr, cancel_for(&invite)).await;
    assert_eq!(
        answered.status.code(),
        200,
        "the CANCEL still matched a transaction of ours, so it is answered 200"
    );

    // Nothing else: the invitation was not ended, so no 487 chased the 2xx …
    //
    // A definition of silence: how long a hole has to be before "no event followed" is true. The
    // assertion is negative, so load lengthens the window and can only make it fail (`X-44`).
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        events.try_recv().is_none(),
        "a CANCEL after the final response must end nothing — BYE is the request for that"
    );

    // … and the call is still running.
    served.abort();
    let call = tokio::time::timeout(Duration::from_secs(5), served)
        .await
        .expect("the serve task stops")
        .expect_err("aborted");
    assert!(call.is_cancelled(), "the serve loop was still running");
}

/// A second copy of the same CANCEL is a retransmission, and the server transaction absorbs it:
/// the same `200` comes back and nothing is cancelled twice.
#[tokio::test]
async fn a_replayed_cancel_draws_the_same_answer_and_nothing_more() {
    let (callee, callee_incoming) = endpoint().await;
    let callee_addr = callee.local_addr();
    let mut pumped = pump(&callee, callee_incoming);
    let (peer, _peer_incoming) = endpoint().await;

    let invite = invite(&peer, "again@sipx", "caller", "z9hG4bK-s23-replay");
    let mut invited = peer
        .send(invite.clone(), Target::udp(callee_addr))
        .await
        .expect("the INVITE goes out");
    let mut invitation = pumped.invitation().await;
    let mut events = invitation
        .events()
        .expect("an invitation has one event stream");

    let first = ask(&peer, callee_addr, cancel_for(&invite)).await;
    assert_eq!(first.status.code(), 200);
    let terminated = tokio::time::timeout(Duration::from_secs(5), invited.final_response())
        .await
        .expect("the INVITE is answered")
        .expect("a final response");
    assert_eq!(terminated.status.code(), 487);
    assert!(
        matches!(
            events.recv().await,
            Some(sipx_call::CallEvent::Ended(
                sipx_call::EndCause::RemoteCancel
            ))
        ),
        "the first CANCEL is the one that ends the invitation"
    );

    // The same CANCEL again. Its own transaction is gone by now on this side only if 32 seconds
    // have passed, which they have not, so the answer is the one already sent.
    let again = ask(&peer, callee_addr, cancel_for(&invite)).await;
    assert_eq!(
        again.status.code(),
        200,
        "a replayed CANCEL is answered, not treated as a second cancellation"
    );
    assert!(invitation.is_cancelled());

    // "Nothing more" is the half of the name that needs an assertion of its own: the invitation
    // ended once, so there is one event and no second `487` behind it. Measured, so that the
    // claim is not taken on trust: mutating the dispatcher's "already ended" guard away does not
    // fail this test, because the copy is absorbed by the server transaction and never reaches
    // the dispatcher at all. It fails `a_cancel_after_the_answer_does_not_tear_the_dialog_down`,
    // which is where that guard is actually held to account. This assertion pins the layering —
    // if the absorption below ever stops happening, the guard above has to catch the copy.
    //
    // The window is a definition of silence: how long a hole has to be before "no second event"
    // is true. Negative, so load can only make it fail (`X-44`).
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        events.try_recv().is_none(),
        "an invitation is cancelled once, however many copies of the CANCEL arrive"
    );
}

/// An invitation this stack never issued is not made cancellable by answering it: the CANCEL
/// gets 481, and `answer` still works on the invitation that *is* live. A CANCEL from a stranger
/// has to get the branch, the sent-by, the `Call-ID` *and* the `From` tag right before it names
/// anything of ours — §9.2's transaction match plus §9.1's requirement that a CANCEL carry the
/// INVITE's own dialog identifiers.
#[tokio::test]
async fn a_cancel_from_a_third_party_does_not_reach_someone_elses_invitation() {
    let (callee, callee_incoming) = endpoint().await;
    let callee_addr = callee.local_addr();
    let mut pumped = pump(&callee, callee_incoming);
    let (peer, _peer_incoming) = endpoint().await;
    let (stranger, _stranger_incoming) = endpoint().await;

    let mine = invite(&peer, "mine@sipx", "caller", "z9hG4bK-s23-mine");
    let _invited = peer
        .send(mine.clone(), Target::udp(callee_addr))
        .await
        .expect("the INVITE goes out");
    let invitation = pumped.invitation().await;

    // Same branch, but a different caller: it comes from the stranger's own socket and carries
    // the stranger's own `From` tag, so it names no transaction of ours however well it guessed
    // the branch.
    let spoofed = invite(&stranger, "mine@sipx", "not-the-caller", "z9hG4bK-s23-mine");
    let answered = ask(&stranger, callee_addr, cancel_for(&spoofed)).await;
    assert_eq!(answered.status.code(), 481);
    assert!(
        !invitation.is_cancelled(),
        "a third party must not be able to end someone else's invitation"
    );
    drop(answer(&callee, invitation.request(), loopback()).await);
}

/// The `From` tag on its own, with everything §17.2.3 matches on correct.
///
/// The vector above changes the sent-by as well as the caller, so it would pass on the transaction
/// match alone. This one does not: the CANCEL leaves the same socket, on the same branch, for the
/// same method — so §9.2's match finds the invitation — and differs only in the dialog identifiers
/// §9.1 requires a CANCEL to copy from the INVITE. Refusing it is what makes that term
/// load-bearing rather than decorative.
#[tokio::test]
async fn a_cancel_on_the_right_transaction_from_the_wrong_dialog_is_refused() {
    let (callee, callee_incoming) = endpoint().await;
    let callee_addr = callee.local_addr();
    let mut pumped = pump(&callee, callee_incoming);
    let (peer, _peer_incoming) = endpoint().await;

    let mine = invite(&peer, "identifiers@sipx", "caller", "z9hG4bK-s23-ident");
    let mut invited = peer
        .send(mine.clone(), Target::udp(callee_addr))
        .await
        .expect("the INVITE goes out");
    let invitation = pumped.invitation().await;

    // Same peer, same branch, same method — and a `From` tag that belongs to no invitation here.
    let elsewhere = invite(
        &peer,
        "identifiers@sipx",
        "somebody-else",
        "z9hG4bK-s23-ident",
    );
    let answered = ask(&peer, callee_addr, cancel_for(&elsewhere)).await;
    assert_eq!(
        answered.status.code(),
        481,
        "§9.1: a CANCEL carries the INVITE's own Call-ID and From, or it names nothing"
    );
    assert!(!invitation.is_cancelled(), "the invitation is untouched");

    let call = invitation.answer(&callee, loopback()).await;
    assert!(call.is_ok(), "and it can still be answered");
    let accepted = tokio::time::timeout(Duration::from_secs(5), invited.final_response())
        .await
        .expect("the INVITE is answered")
        .expect("a final response");
    assert_eq!(accepted.status.code(), 200);
}

/// The application is *told*, not left to poll: a host that is ringing has to stop ringing, and
/// nothing wakes it up unless the end of the invitation arrives on a stream it can await.
///
/// The vocabulary is `C-3`'s, deliberately — `CallEvent::Ended`, with a cause that says a CANCEL
/// and not a BYE did it — rather than a second channel meaning the same thing for the half of a
/// call's life that happens before there is a `Call`.
#[tokio::test]
async fn a_ringing_host_is_told_the_caller_gave_up_and_why() {
    let (callee, callee_incoming) = endpoint().await;
    let callee_addr = callee.local_addr();
    let mut pumped = pump(&callee, callee_incoming);
    let (peer, _peer_incoming) = endpoint().await;

    let invite = invite(&peer, "told@sipx", "caller", "z9hG4bK-s23-told");
    let _invited = peer
        .send(invite.clone(), Target::udp(callee_addr))
        .await
        .expect("the INVITE goes out");

    let mut invitation = pumped.invitation().await;
    let mut events = invitation
        .events()
        .expect("an invitation has one event stream");
    assert!(
        invitation.events().is_none(),
        "the stream is handed out exactly once, as a call's is"
    );

    // The host is ringing, and is waiting on the stream rather than polling the invitation.
    let ringing = ring(&callee, invitation.request(), 180, "Ringing", false)
        .await
        .expect("rings");
    let waiting = tokio::spawn(async move { events.recv().await });

    let _ = peer
        .send(cancel_for(&invite), Target::udp(callee_addr))
        .await
        .expect("the CANCEL goes out");

    let event = tokio::time::timeout(Duration::from_secs(5), waiting)
        .await
        .expect("the host is woken up rather than left waiting")
        .expect("the waiting task finished");
    assert!(
        matches!(
            event,
            Some(sipx_call::CallEvent::Ended(
                sipx_call::EndCause::RemoteCancel
            ))
        ),
        "the invitation ends with the cause that says a CANCEL did it: {event:?}"
    );

    // Which is the host's cue to stop ringing, and it is then out of options anyway.
    drop(ringing);
    assert!(
        matches!(
            invitation.answer(&callee, loopback()).await,
            Err(sipx_call::Error::InvitationCancelled)
        ),
        "an invitation that was cancelled must not be answerable afterwards"
    );
}

/// A-4: cancellation is an input to early dialing, not a dropped future. Once the INVITE has
/// left, the API retains ownership long enough to wait for the provisional RFC 3261 §9.1 requires
/// and then sends the matching CANCEL.
#[tokio::test]
async fn cancelling_while_early_dial_waits_withdraws_the_invitation() {
    let (callee, callee_incoming) = endpoint().await;
    let mut pumped = pump(&callee, callee_incoming);
    let (source_endpoint, _source_incoming) = endpoint().await;
    let cancelled = Arc::new(Notify::new());
    let cancellation = Arc::clone(&cancelled);
    let target = Target::udp(callee.local_addr());
    let dialing = tokio::spawn(async move {
        sipx_call::call::dial_early_until(
            &source_endpoint,
            target,
            &callee_uri(),
            &DialOptions::new("<sip:caller@example.net>", loopback()),
            cancellation.notified_owned(),
        )
        .await
    });

    let mut invitation = pumped.invitation().await;
    let mut events = invitation
        .events()
        .expect("the pending invitation has a cancellation stream");
    cancelled.notify_one();
    let _ringing = ring(&callee, invitation.request(), 180, "Ringing", false)
        .await
        .expect("the provisional permits cancellation");

    let (result, ended) = tokio::join!(
        tokio::time::timeout(Duration::from_secs(10), dialing),
        tokio::time::timeout(Duration::from_secs(10), events.recv()),
    );
    assert!(matches!(
        result
            .expect("the cancellation-safe dial finishes")
            .expect("the dial task finishes"),
        Err(Error::Cancelled(duration)) if duration == Duration::ZERO
    ));
    assert!(matches!(
        ended.expect("the target is told to stop ringing"),
        Some(sipx_call::CallEvent::Ended(
            sipx_call::EndCause::RemoteCancel
        ))
    ));
    assert!(invitation.is_cancelled());
}

/// The cancellation-safe ownership continues after the early handle is returned and while its
/// final answer is outstanding.
#[tokio::test]
async fn cancelling_while_an_early_handle_awaits_confirmation_withdraws_it() {
    let (callee, callee_incoming) = endpoint().await;
    let mut pumped = pump(&callee, callee_incoming);
    let (source_endpoint, _source_incoming) = endpoint().await;
    let target = Target::udp(callee.local_addr());
    let dialing = tokio::spawn(async move {
        sipx_call::dial_early(
            &source_endpoint,
            target,
            &callee_uri(),
            &DialOptions::new("<sip:caller@example.net>", loopback()),
        )
        .await
        .expect("the early handle is returned")
    });

    let mut invitation = pumped.invitation().await;
    let mut events = invitation
        .events()
        .expect("the pending invitation has a cancellation stream");
    let _ringing = ring(&callee, invitation.request(), 180, "Ringing", false)
        .await
        .expect("the early dialog is established");
    let dialing = dialing.await.expect("the dialing task finishes");
    let cancelled = Arc::new(Notify::new());
    let cancellation = Arc::clone(&cancelled);
    let confirming =
        tokio::spawn(async move { dialing.answered_until(cancellation.notified_owned()).await });
    cancelled.notify_one();

    let (result, ended) = tokio::join!(
        tokio::time::timeout(Duration::from_secs(10), confirming),
        tokio::time::timeout(Duration::from_secs(10), events.recv()),
    );
    assert!(matches!(
        result
            .expect("confirmation cancellation finishes")
            .expect("the confirmation task finishes"),
        Err(Error::Cancelled(duration)) if duration == Duration::ZERO
    ));
    assert!(matches!(
        ended.expect("the target is told to stop ringing"),
        Some(sipx_call::CallEvent::Ended(
            sipx_call::EndCause::RemoteCancel
        ))
    ));
    assert!(invitation.is_cancelled());
}
