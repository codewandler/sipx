//! The downstream contract of the application and transaction harnesses.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use bytes::Bytes;
use sipx_call::{CallEvent, DialOptions};
use sipx_sip::build::RequestBuilder;
use sipx_sip::{HeaderName, Host, HostName, Method, Uri};
use sipx_testkit::call::{CallHarness, HarnessError, TransactionHarness};
use sipx_testkit::link::{Faults, Link, Side};
use sipx_testkit::time::Virtual;

fn uri(name: &str) -> Uri {
    Uri::sip(Host::Name(
        HostName::new(name.to_owned()).expect("valid host"),
    ))
}

fn request(method: &Method, branch: &str, call_id: &str) -> sipx_sip::Request {
    RequestBuilder::new(method.clone(), uri("callee.example"))
        .header(
            HeaderName::Via,
            Bytes::from(format!(
                "SIP/2.0/UDP caller.example;branch=z9hG4bK-{branch}"
            )),
        )
        .expect("valid Via")
        .header(HeaderName::To, Bytes::from_static(b"<sip:callee.example>"))
        .expect("valid To")
        .header(
            HeaderName::From,
            Bytes::from_static(b"<sip:caller@example.net>;tag=caller"),
        )
        .expect("valid From")
        .header(HeaderName::CallId, Bytes::from(call_id.to_owned()))
        .expect("valid Call-ID")
        .cseq(1, method)
        .expect("valid CSeq")
        .max_forwards(70)
        .build()
}

#[tokio::test]
async fn the_public_harness_establishes_real_calls_and_delivers_the_ack() {
    let mut harness = CallHarness::new().expect("runtime is entered");
    let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let options = DialOptions::new("sip:caller@example.net", loopback);

    let pending = harness
        .dial(uri("callee.example"), options)
        .await
        .expect("real dial produced an invitation");
    assert_eq!(pending.invitation().request.method, Method::Invite);

    let mut established = pending
        .answer(loopback)
        .await
        .expect("real answer produced two calls and a matching ACK");
    let mut originating_events = established.caller.events().expect("caller event stream");
    let mut answering_events = established.callee.events().expect("callee event stream");
    assert!(matches!(
        originating_events.recv().await,
        Some(CallEvent::Answered)
    ));
    assert!(matches!(
        answering_events.recv().await,
        Some(CallEvent::Answered)
    ));
}

#[tokio::test]
async fn each_pending_call_owns_only_its_own_invitation_and_response_stream() {
    let mut harness = CallHarness::new().expect("runtime is entered");
    let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);

    for callee in ["first.example", "second.example"] {
        let pending = harness
            .dial(
                uri(callee),
                DialOptions::new("sip:caller@example.net", loopback),
            )
            .await
            .expect("this call produced its invitation");
        assert_eq!(
            pending.invitation().request.uri.to_bytes(),
            uri(callee).to_bytes()
        );
        pending
            .answer(loopback)
            .await
            .expect("this call established independently");
    }
}

#[test]
fn construction_without_a_runtime_is_a_typed_error() {
    let error = CallHarness::new().expect_err("no Tokio runtime is entered");
    assert!(matches!(
        error,
        HarnessError::Transport(sipx_transport::Error::RuntimeUnavailable)
    ));
}

#[tokio::test]
async fn a_pre_signalling_dial_error_is_returned_without_waiting_for_an_invitation() {
    let mut harness = CallHarness::new().expect("runtime is entered");
    let unspecified = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
    let result = tokio::time::timeout(
        Duration::from_secs(1), // failure bound: how long a broken dial may hold the test
        harness.dial(
            uri("callee.example"),
            DialOptions::new("sip:caller@example.net", unspecified),
        ),
    )
    .await
    .expect("pre-signalling failure stayed inside its bound")
    .expect_err("an unspecified media address is refused");
    assert!(matches!(
        result,
        HarnessError::Call(sipx_call::Error::UnspecifiedMediaAddress)
    ));
}

fn seed_that_drops_only_the_first_send() -> u64 {
    (0..10_000)
        .find(|seed| {
            let mut link = Link::<Virtual>::new(*seed, Faults::losing(0.5));
            link.send(Side::Left, Bytes::from_static(b"first"), Virtual::epoch());
            link.send(Side::Left, Bytes::from_static(b"second"), Virtual::epoch());
            link.dropped() == 1 && link.in_flight() == 1
        })
        .expect("a deterministic seed exists")
}

#[test]
fn one_large_advance_is_equivalent_to_chronological_small_steps() {
    let seed = seed_that_drops_only_the_first_send();
    let faults = Faults {
        loss: 0.5,
        latency: Duration::from_millis(100),
        ..Faults::default()
    };
    let mut large = TransactionHarness::new(seed, faults);
    let mut small = TransactionHarness::new(seed, faults);
    large
        .place(request(&Method::Invite, "large", "same@example.net"))
        .expect("transaction starts");
    small
        .place(request(&Method::Invite, "large", "same@example.net"))
        .expect("transaction starts");

    large.advance(Duration::from_millis(600));
    small.advance(Duration::from_millis(500));
    small.advance(Duration::from_millis(100));

    assert_eq!(large.invitation().is_some(), small.invitation().is_some());
    assert!(
        large.invitation().is_some(),
        "the retransmission arrived at 600ms"
    );
    assert_eq!(large.dropped(), small.dropped());
}

#[test]
fn transaction_observations_are_scoped_to_the_latest_exchange() {
    let mut harness = TransactionHarness::perfect();
    harness
        .place(request(&Method::Invite, "first", "first@example.net"))
        .expect("first transaction starts");
    harness.answer_ok().expect("first invitation answers");
    assert!(harness.invitation().is_some());
    assert!(harness.response().is_some());

    harness
        .place(request(&Method::Options, "second", "second@example.net"))
        .expect("second transaction starts");
    assert!(harness.invitation().is_none(), "the first INVITE is stale");
    assert!(harness.response().is_none(), "the first 200 is stale");
}
