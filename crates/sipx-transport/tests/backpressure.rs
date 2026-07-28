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
use sipx_sip::{HeaderName, Host, HostName, Method, Uri};
use sipx_transport::{Config, Handle, Incoming, Target, bind, new_branch};
use tokio::sync::mpsc::Receiver;

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

    // Give the loop time to process what it can.
    tokio::time::sleep(Duration::from_millis(300)).await;

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
    assert!(busy.shed().requests > 0, "and the refusal is counted");
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
