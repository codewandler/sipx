//! Graceful endpoint drain (T-29).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::similar_names
)]

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::{HeaderName, Host, HostName, Method, StatusCode, Uri};
use sipx_transport::{Error, Target, TransportKind, in_process_pair};

fn uri() -> Uri {
    Uri::sip(Host::Name(
        HostName::new("drain.invalid").expect("valid host"),
    ))
}

fn request(method: &Method, tagged: bool) -> sipx_sip::Request {
    let to = if tagged {
        "<sip:callee@drain.invalid>;tag=callee"
    } else {
        "<sip:callee@drain.invalid>"
    };
    RequestBuilder::new(method.clone(), uri())
        .header(
            HeaderName::Via,
            Bytes::from_static(b"SIP/2.0/TCP caller.invalid;branch=z9hG4bKdrain"),
        )
        .expect("Via")
        .header(HeaderName::To, Bytes::from_static(to.as_bytes()))
        .expect("To")
        .header(
            HeaderName::From,
            Bytes::from_static(b"<sip:caller@drain.invalid>;tag=caller"),
        )
        .expect("From")
        .header(HeaderName::CallId, Bytes::from_static(b"drain@invalid"))
        .expect("Call-ID")
        .cseq(1, method)
        .expect("CSeq")
        .header(
            HeaderName::Contact,
            Bytes::from_static(b"<sip:caller@drain.invalid>"),
        )
        .expect("Contact")
        .max_forwards(70)
        .build()
}

fn assert_pending<F: Future>(future: Pin<&mut F>) {
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    assert!(matches!(future.poll(&mut context), Poll::Pending));
}

/// D3: endpoint completion is the transaction's terminal response, not elapsed wall time.
#[tokio::test]
async fn settled_waits_for_the_last_transaction_to_reach_terminal_state() {
    let ((caller, _), (callee, mut incoming)) = in_process_pair(8).expect("runtime is entered");
    let mut responses = caller
        .send(
            request(&Method::Options, false),
            Target::new(callee.local_addr(), TransportKind::Tcp),
        )
        .await
        .expect("request is admitted");
    let received = incoming.recv().await.expect("request reaches callee");

    let settled = callee.settled();
    tokio::pin!(settled);
    assert_pending(settled.as_mut());

    let response = ResponseBuilder::to_request(
        &received.request,
        StatusCode::new(200).expect("status"),
        "OK",
    )
    .expect("response")
    .build();
    callee
        .respond(&received.key, response)
        .await
        .expect("final response terminates transaction");
    responses
        .final_response()
        .await
        .expect("caller observes final response");

    tokio::time::timeout(Duration::from_secs(5), settled)
        .await
        .expect("a bound on failure: terminal state releases the barrier")
        .expect("endpoint remains open");
    caller.shutdown().await;
    callee.shutdown().await;
}

#[tokio::test]
async fn begin_drain_refuses_a_new_outbound_dialog_before_transaction_creation() {
    let ((caller, _), (callee, mut incoming)) = in_process_pair(8).expect("runtime is entered");
    caller.begin_drain();

    let error = caller
        .send(
            request(&Method::Invite, false),
            Target::new(callee.local_addr(), TransportKind::Tcp),
        )
        .await
        .expect_err("a fresh dialog is outside the drain barrier");
    assert!(matches!(error, Error::EndpointDraining));
    assert_eq!(caller.outstanding().await.expect("observable"), 0);

    let mut existing = caller
        .send(
            request(&Method::Invite, true),
            Target::new(callee.local_addr(), TransportKind::Tcp),
        )
        .await
        .expect("a tagged in-dialog INVITE remains legal");
    let received = incoming.recv().await.expect("re-INVITE reaches peer");
    let response = ResponseBuilder::to_request(
        &received.request,
        StatusCode::new(200).expect("status"),
        "OK",
    )
    .expect("response")
    .build();
    callee
        .respond(&received.key, response)
        .await
        .expect("re-INVITE is answered");
    existing
        .final_response()
        .await
        .expect("in-dialog transaction settles");

    caller.shutdown().await;
    callee.shutdown().await;
}
