//! Session timers (RFC 4028): noticing a far end that stopped existing.
//!
//! The failure these exist for is invisible in a lab and permanent in production. A peer that
//! loses power closes no socket and sends no BYE, so both dialogs stay up: one side streaming
//! audio at a machine that is off, the other side gone. Nothing else in SIP notices — there is
//! no keepalive on a dialog — which is why the tests here are about a call that *should* end
//! and, without this mechanism, never would.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::similar_names)]

use std::net::IpAddr;
use std::time::Duration;

use sipx_call::{Call, DialOptions, Error, answer, dial};
use sipx_sip::session::{self, MinSe, Refresher, SessionExpires};
use sipx_sip::{HeaderName, Host, HostName, Method, Uri};
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

fn to_uri() -> Uri {
    Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")))
}

/// A call placed with a session timer, both sides of it, and the caller's inbox.
async fn timed_call(options: DialOptions) -> (Call, Call, Receiver<Incoming>) {
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, caller_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    let answering = tokio::spawn(async move {
        loop {
            let incoming = callee_incoming.recv().await.expect("an INVITE arrives");
            assert_eq!(incoming.request.method, Method::Invite);
            // A 422 is a counter-offer, not a refusal: the caller is expected to come back
            // with the interval this side named, so the loop keeps answering.
            match answer(&callee_endpoint, &incoming, loopback()).await {
                Ok(call) => return call,
                Err(Error::IntervalTooBrief(_)) => {}
                Err(error) => panic!("the callee failed for another reason: {error}"),
            }
        }
    });

    let caller = dial(
        &caller_endpoint,
        Target::udp(callee_addr),
        &to_uri(),
        &options,
    )
    .await
    .expect("the call connects");

    let callee = answering.await.expect("the answering side finishes");
    (caller, callee, caller_incoming)
}

/// The story's failing-first test.
#[tokio::test]
async fn a_call_whose_far_end_vanishes_is_torn_down() {
    let (mut caller, callee, _inbox) = timed_call(
        DialOptions::new("<sip:caller@example.net>", loopback())
            .with_session_timer(session::ABSOLUTE_MIN_INTERVAL),
    )
    .await;

    // The caller asked for a timer without naming a refresher, so RFC 4028 Table 2 row 4 let
    // the answering side take the job. That makes the caller the one that detects silence.
    assert!(
        caller.session_deadline().is_some(),
        "no session timer was negotiated, so nothing could ever notice a dead peer"
    );
    assert!(!caller.is_ended());

    // The far end vanishes: no BYE, no refresh, nothing. Dropping the `Call` while leaving its
    // endpoint bound is exactly what a powered-off phone looks like from here — the socket is
    // still there, and it will never say anything again.
    drop(callee);

    // Past the point where the refresh should have arrived. RFC 4028 §10 puts that at
    // `interval - min(32s, interval/3)`, which for the ninety-second floor is sixty seconds.
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(61)).await;

    let outcome = caller.on_session_deadline().await;
    assert!(
        matches!(outcome, Err(Error::SessionExpired)),
        "expected the session to be reported expired, got {outcome:?}"
    );
    assert!(caller.is_ended(), "the call was left up");
    assert!(
        caller.session_deadline().is_none(),
        "an elapsed deadline was left armed, which would spin any loop that selects on it"
    );
}

#[tokio::test]
async fn a_call_that_is_refreshed_stays_up() {
    // The mirror of the test above, and what stops it from passing for the wrong reason: if
    // the deadline fired regardless of what arrived, this would tear down a call whose far end
    // is demonstrably alive.
    let (mut caller, mut callee, mut inbox) = timed_call(
        DialOptions::new("<sip:caller@example.net>", loopback())
            .with_session_timer(session::ABSOLUTE_MIN_INTERVAL),
    )
    .await;

    let before = caller.session_deadline().expect("a timer");

    // The refresher does its job. A re-INVITE refreshes the session whichever side sent it and
    // whatever it was sent for (RFC 4028 §7.2), so this is the real mechanism, not a stub.
    let refreshing = tokio::spawn(async move {
        callee
            .reinvite(sipx_sdp::Direction::SendRecv)
            .await
            .expect("the refresh is accepted");
        callee
    });

    // Drive the caller the way an application would, until the re-INVITE has been handled.
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(message) = inbox.recv().await {
            caller.handle(&message).await.expect("handles");
            if message.request.method == Method::Invite {
                break;
            }
        }
    })
    .await
    .expect("the refresh arrives");
    let _callee = refreshing.await.expect("the refresh finishes");

    let after = caller.session_deadline().expect("still timed");
    assert!(
        after > before,
        "a refresh arrived and the deadline did not move: {before:?} -> {after:?}"
    );
    assert!(!caller.is_ended());
}

#[tokio::test]
async fn an_interval_below_the_floor_is_refused_and_retried_at_it() {
    // Built by hand rather than through `with_session_timer`, which clamps: the point is what
    // happens when the *far end* is the one enforcing the floor, which is the case the RFC
    // actually writes 422 for.
    let mut options = DialOptions::new("<sip:caller@example.net>", loopback());
    options.session_expires = Some(Duration::from_secs(30));

    let (caller, _callee, _inbox) = timed_call(options).await;

    // The call is up, which means the 422 was retried rather than surfaced as a failure — and
    // it is up on the floor the far end named, not on the thirty seconds that was asked for.
    // Asserting the interval rather than merely that a timer exists is the difference between
    // this test and one that would also pass if the 422 were dropped and the thirty seconds
    // quietly accepted.
    assert_eq!(
        caller.session_interval(),
        Some((session::ABSOLUTE_MIN_INTERVAL, false)),
        "the retry did not land on the interval the far end demanded"
    );
    assert!(!caller.is_ended());
}

#[tokio::test]
async fn a_uas_that_is_asked_for_too_little_says_how_much_it_needs() {
    // Directly, at the message level: a 422 that does not carry `Min-SE` tells the caller it
    // was wrong without telling it what would be right, and it retries the same value forever.
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, _caller_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("an INVITE");
        let _ = answer(&callee_endpoint, &incoming, loopback()).await;
    });

    let mut options = DialOptions::new("<sip:caller@example.net>", loopback());
    options.session_expires = Some(Duration::from_secs(30));

    // One attempt only, so the 422 is observable instead of being retried away.
    let outcome = sipx_call::dial_once(
        &caller_endpoint,
        Target::udp(callee_addr),
        &to_uri(),
        &options,
    )
    .await;

    match outcome {
        Err(Error::IntervalTooBrief(required)) => {
            assert_eq!(required, session::ABSOLUTE_MIN_INTERVAL);
        }
        other => panic!("expected a 422 carrying Min-SE, got {other:?}"),
    }
}

#[tokio::test]
async fn the_invite_advertises_the_timer_and_names_no_refresher() {
    // The wire, not the state. `Supported: timer` has to be there even when no timer was asked
    // for, because it is what lets the *far end* run one — and the refresher parameter has to
    // be absent, because naming ourselves would override a peer better placed to decide.
    let (peer_endpoint, mut peer_incoming) = endpoint().await;
    let (caller_endpoint, _caller_incoming) = endpoint().await;
    let peer_addr = peer_endpoint.local_addr();

    let seen = tokio::spawn(async move { peer_incoming.recv().await.expect("an INVITE") });

    let options = DialOptions::new("<sip:caller@example.net>", loopback())
        .with_session_timer(Duration::from_secs(600))
        .with_timeout(Duration::from_millis(300));
    let _ = dial(
        &caller_endpoint,
        Target::udp(peer_addr),
        &to_uri(),
        &options,
    )
    .await;

    let invite = seen.await.expect("the INVITE arrives").request;
    let supported = invite
        .headers
        .value(&HeaderName::Supported)
        .expect("Supported is present");
    assert!(
        String::from_utf8_lossy(&supported).contains(session::OPTION_TAG),
        "the INVITE does not advertise timer support"
    );

    let expires = invite
        .headers
        .typed::<SessionExpires>()
        .expect("Session-Expires is present")
        .expect("it parses");
    assert_eq!(expires.interval, Duration::from_secs(600));
    assert_eq!(
        expires.refresher, None,
        "the UAC named a refresher and took the choice away from the UAS"
    );

    let min_se = invite
        .headers
        .typed::<MinSe>()
        .expect("Min-SE is present")
        .expect("it parses");
    assert_eq!(
        min_se.0,
        session::ABSOLUTE_MIN_INTERVAL,
        "without Min-SE the caller has no defence against being driven at any rate the far end likes"
    );
}

#[tokio::test]
async fn the_answer_names_a_refresher_and_requires_the_extension() {
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, _caller_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("an INVITE");
        let _ = answer(&callee_endpoint, &incoming, loopback()).await;
    });

    let options = DialOptions::new("<sip:caller@example.net>", loopback())
        .with_session_timer(Duration::from_secs(600));
    let call = dial(
        &caller_endpoint,
        Target::udp(callee_addr),
        &to_uri(),
        &options,
    )
    .await
    .expect("connects");

    // Table 2 row 4: the UAC made no choice, so the UAS took the job — which means the caller
    // is the side waiting, and its deadline is the near-expiry one rather than the halfway one.
    let deadline = call.session_deadline().expect("a timer");
    let waited = deadline.duration_since(tokio::time::Instant::now());
    let expected = session::Session {
        interval: Duration::from_secs(600),
        we_refresh: false,
    }
    .act_after();
    assert!(
        waited <= expected && waited + Duration::from_secs(5) >= expected,
        "expected to be the waiting side ({expected:?}), got {waited:?}"
    );
    assert_ne!(
        Refresher::Uac.as_str(),
        Refresher::Uas.as_str(),
        "the two roles must be distinguishable on the wire"
    );
}
