//! What happens when the application stops keeping up.
//!
//! Shedding under load is a policy and a reasonable one. Shedding *invisibly* is not: a stack whose
//! premise is that its failure modes are testable cannot have a path where a request disappears
//! with no counter, no log and no response. These tests are about the difference.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::RequestBuilder;
use sipx_sip::headers::{OcParameter, OverloadAlgorithm, Via};
use sipx_sip::{HeaderName, Host, HostName, Method, Uri};
use sipx_transport::{Config, Handle, Incoming, Target, bind, new_branch};
use tokio::sync::mpsc::Receiver;

/// How long a test here waits for the receive loop to have shed something before concluding it
/// never will (`X-29`). A bound on failure, not a window to measure in.
const SHEDDING_BOUND: Duration = Duration::from_secs(10);

/// Wait until something has happened, rather than sleeping and assuming it has (`X-29`).
///
/// Load can only lengthen the wait, and "it never happened" fails with a message that says so
/// instead of flaking. `X-28` waited for a *quantity* of audio; this waits for an *event*, so the
/// shape is a deadline loop on the condition.
async fn until(within: Duration, what: &str, mut condition: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + within;
    while !condition().await {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// An endpoint whose application queue holds exactly one message.
///
/// One, so the second request has nowhere to go. The alternative — filling a 1024-deep queue —
/// would test the same code path and take a thousand times as long to say so.
async fn saturated() -> (Handle, Receiver<Incoming>) {
    let mut config = Config::new("127.0.0.1:0".parse().expect("valid"));
    config.capacity = 1;
    bind(config).await.expect("binds")
}

fn to_uri() -> Uri {
    Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")))
}

/// A request built by hand so the test controls the method and the transaction it keys.
fn request(sender: &Handle, method: &Method, call_id: &'static str) -> sipx_sip::Request {
    RequestBuilder::new(method.clone(), to_uri())
        .header(
            HeaderName::Via,
            Bytes::from(format!(
                "SIP/2.0/UDP {};rport;branch={}",
                sender.sent_by_for(sipx_transport::TransportKind::Udp),
                new_branch()
            )),
        )
        .expect("via")
        .header(HeaderName::To, Bytes::from_static(b"<sip:callee.example>"))
        .expect("to")
        .header(
            HeaderName::From,
            Bytes::from_static(b"<sip:caller@example.net>;tag=abc"),
        )
        .expect("from")
        .header(HeaderName::CallId, Bytes::from_static(call_id.as_bytes()))
        .expect("call-id")
        .cseq(1, method)
        .expect("cseq")
        .max_forwards(70)
        .build()
}

/// The story's failing-first test.
///
/// Before this, the endpoint's delivery path ended in `let _ = try_send(…)`: the request was gone,
/// nothing was logged, and no counter moved. The point is not that shedding is wrong — it is that
/// shedding has to leave a trace.
#[tokio::test]
async fn a_request_dropped_for_backpressure_is_counted() {
    let (busy, mut incoming) = saturated().await;
    let (sender, _sender_incoming) = bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");
    let busy_addr = busy.local_addr();

    assert!(!busy.shed().any(), "nothing has been shed yet");

    // Fill the queue and leave it full: the receiver is never read from. `request` mints a fresh
    // branch every time, so these are eight transactions and not eight retransmissions of one —
    // which the transaction layer would absorb, shedding nothing.
    for _ in 0..8u32 {
        let _ = sender
            .send_directly(
                request(&sender, &Method::Invite, "backpressure@sipx"),
                Target::udp(busy_addr),
            )
            .await;
    }

    // Wait for the loop to have shed something, rather than giving it 300 ms and assuming it did
    // (`X-29`). Eight requests into a one-deep queue must produce at least one drop; how long
    // that takes is a property of the machine, not of the shedding.
    until(
        SHEDDING_BOUND,
        "eight requests into a one-deep queue shed nothing",
        async || busy.shed().any(),
    )
    .await;

    let shed = busy.shed();
    assert!(
        shed.any(),
        "requests were dropped for backpressure and nothing counted them: {shed:?}"
    );
    assert_eq!(
        shed.total(),
        shed.requests + shed.acks + shed.unmatched,
        "the total must be the sum of its parts"
    );

    // The queue really is full and really did receive something — otherwise this test would pass
    // by never having delivered anything at all.
    assert!(
        incoming.try_recv().is_ok(),
        "the one slot in the queue should hold the first request"
    );
}

/// A shed request gets a `503`, not silence. A peer that is told to back off behaves better than
/// one that is ignored — it stops retransmitting into a queue that is still full.
#[tokio::test]
async fn a_shed_request_is_refused_rather_than_ignored() {
    let (busy, _incoming) = saturated().await;
    let (sender, _sender_incoming) = bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");
    let busy_addr = busy.local_addr();

    // Two requests through the transaction layer, so the second finds the queue full. The first
    // is sent with `send` rather than `send_directly` so a response can be observed.
    let mut refused = None;
    for _ in 0..6u32 {
        let mut responses = sender
            .send(
                request(&sender, &Method::Options, "refusal@sipx"),
                Target::udp(busy_addr),
            )
            .await
            .expect("sends");
        if let Ok(Some(response)) =
            tokio::time::timeout(Duration::from_millis(400), responses.final_response()).await
            && response.status.code() == 503
        {
            refused = Some(response);
            break;
        }
    }

    let response = refused.expect("a saturated endpoint should answer 503 rather than say nothing");
    assert_eq!(response.status.code(), 503);
    assert!(
        response.headers.value(&HeaderName::RetryAfter).is_some(),
        "a 503 without Retry-After tells a peer to back off for an unspecified time"
    );
    let overload = response
        .headers
        .typed::<Via>()
        .expect("503 has a Via")
        .expect("Via parses")
        .overload()
        .expect("overload parameters parse");
    assert_eq!(overload.oc, Some(OcParameter::Value(100)));
    assert_eq!(overload.algorithms, vec![OverloadAlgorithm::Loss]);
    assert!(
        overload.validity.is_some_and(|validity| {
            !validity.is_zero() && validity <= Duration::from_millis(500)
        }),
        "the 503 reports the detector's remaining validity"
    );
    assert!(overload.sequence.is_some(), "a server report is sequenced");
    assert!(busy.shed().requests > 0, "and the refusal is counted");
}

#[tokio::test]
async fn an_application_response_reports_active_control_while_the_queue_remains_saturated() {
    let (busy, mut incoming) = saturated().await;
    let (sender, _sender_incoming) = bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");
    let busy_addr = busy.local_addr();

    let mut responses = sender
        .send(
            request(&sender, &Method::Options, "answer-during-overload@sipx"),
            Target::udp(busy_addr),
        )
        .await
        .expect("first request sends");
    let answerable = tokio::time::timeout(Duration::from_secs(2), incoming.recv())
        .await
        .expect("first request arrives")
        .expect("incoming channel remains open");

    // The first flood request occupies the sole queue slot and later requests activate the
    // queue-full detector. Do not drain that slot: the application is still saturated when it
    // answers the earlier request retained above.
    for _ in 0..8u32 {
        let _ = sender
            .send_directly(
                request(&sender, &Method::Options, "saturated-answer@sipx"),
                Target::udp(busy_addr),
            )
            .await;
    }
    until(
        SHEDDING_BOUND,
        "the full application queue never activated overload feedback",
        async || busy.shed().requests > 0,
    )
    .await;

    let status = sipx_sip::StatusCode::new(200).expect("status");
    let response = sipx_sip::ResponseBuilder::to_request(&answerable.request, status, "OK")
        .expect("response")
        .build();
    busy.respond(&answerable.key, response)
        .await
        .expect("response sends");
    let response = tokio::time::timeout(Duration::from_secs(2), responses.final_response())
        .await
        .expect("response arrives")
        .expect("final response");
    let overload = response
        .headers
        .typed::<Via>()
        .expect("response has Via")
        .expect("Via parses")
        .overload()
        .expect("overload parameters parse");
    assert_eq!(overload.oc, Some(OcParameter::Value(100)));
    assert_eq!(overload.algorithms, vec![OverloadAlgorithm::Loss]);
    assert!(
        overload
            .validity
            .is_some_and(|validity| !validity.is_zero()),
        "an application response must not cancel active queue-full feedback"
    );
    assert!(overload.sequence.is_some());

    sender.shutdown().await;
    busy.shutdown().await;
}

/// An endpoint that is keeping up sheds nothing. Without this the counter could be incremented
/// unconditionally and every test above would still pass.
#[tokio::test]
async fn an_endpoint_that_keeps_up_sheds_nothing() {
    let (calm, mut incoming) = bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");
    let (sender, _sender_incoming) = bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");

    let drain = tokio::spawn(async move {
        let mut seen = 0u32;
        while incoming.recv().await.is_some() {
            seen += 1;
            if seen == 4 {
                break;
            }
        }
        seen
    });

    for _ in 0..4u32 {
        let _ = sender
            .send_directly(
                request(&sender, &Method::Options, "calm@sipx"),
                Target::udp(calm.local_addr()),
            )
            .await;
    }

    let seen = tokio::time::timeout(Duration::from_secs(2), drain)
        .await
        .expect("the application keeps up")
        .expect("the task finishes");
    assert_eq!(seen, 4);
    assert!(
        !calm.shed().any(),
        "an endpoint that delivered everything must not report having shed: {:?}",
        calm.shed()
    );
}

/// A response that answers no transaction this endpoint started.
fn stray_response(target_via: &str, call_id: &'static str) -> Vec<u8> {
    format!(
        "SIP/2.0 200 OK\r\n\
         Via: SIP/2.0/UDP {target_via};branch=z9hG4bK-nothing-matches-this\r\n\
         To: <sip:callee.example>;tag=r\r\n\
         From: <sip:caller@example.net>;tag=abc\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: 1 OPTIONS\r\n\
         Content-Length: 0\r\n\r\n"
    )
    .into_bytes()
}

/// The `T-18` story's failing-first test.
///
/// RFC 3261 §16.7 step 1 requires a stateful proxy that finds no response context to forward the
/// response statelessly. It cannot do that if the endpoint drops the response first — which is
/// what happened, because the unmatched arm forwarded only `Message::Request`.
#[tokio::test]
async fn a_response_matching_no_transaction_reaches_the_application() {
    let (endpoint, _incoming) = bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");
    let mut unmatched = endpoint
        .watch_unmatched(8)
        .await
        .expect("the endpoint accepts a watcher");

    let sender = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("binds");
    sender
        .send_to(
            &stray_response(
                &endpoint.sent_by_for(sipx_transport::TransportKind::Udp),
                "stray@sipx",
            ),
            endpoint.local_addr(),
        )
        .await
        .expect("sends");

    let seen = tokio::time::timeout(Duration::from_secs(2), unmatched.recv())
        .await
        .expect("a response that matches nothing must still reach a watcher")
        .expect("the channel is open");

    assert_eq!(seen.response.status.code(), 200);
    assert_eq!(
        seen.transport,
        sipx_transport::TransportKind::Udp,
        "a decision about forwarding needs to know how it arrived"
    );
    assert_eq!(seen.source.ip().to_string(), "127.0.0.1", "and where from");
    assert_eq!(
        seen.response
            .headers
            .value(&HeaderName::CallId)
            .map(|value| String::from_utf8_lossy(&value).into_owned()),
        Some("stray@sipx".to_owned()),
        "the response is delivered unaltered"
    );
}

/// An endpoint nobody is watching keeps behaving exactly as it did: the response is logged and
/// dropped, no channel exists, and nothing anywhere has to handle a case it has no answer for.
#[tokio::test]
async fn an_endpoint_with_no_watcher_is_undisturbed_by_a_stray_response() {
    let (endpoint, mut incoming) = bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");

    let sender = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("binds");
    sender
        .send_to(
            &stray_response(
                &endpoint.sent_by_for(sipx_transport::TransportKind::Udp),
                "ignored@sipx",
            ),
            endpoint.local_addr(),
        )
        .await
        .expect("sends");

    // Nothing arrives on the request channel — a response is not a request, and widening
    // `Incoming` to carry one would make every user agent handle a case it cannot act on.
    assert!(
        tokio::time::timeout(Duration::from_millis(300), incoming.recv())
            .await
            .is_err(),
        "a stray response must not appear as an incoming request"
    );
    assert!(
        !endpoint.shed().any(),
        "and dropping it is not shedding: nobody wanted it"
    );
}
