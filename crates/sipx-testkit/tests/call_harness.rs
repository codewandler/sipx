//! The downstream contract of the public, socket-free call harness.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use bytes::Bytes;
use sipx_sip::build::RequestBuilder;
use sipx_sip::{HeaderName, Host, HostName, Method, StatusCode, Uri};
use sipx_testkit::call::CallHarness;

fn invite() -> sipx_sip::Request {
    RequestBuilder::new(
        Method::Invite,
        Uri::sip(Host::Name(
            HostName::new("callee.example").expect("valid host"),
        )),
    )
    .header(
        HeaderName::Via,
        Bytes::from_static(b"SIP/2.0/UDP caller.example;branch=z9hG4bK-public-harness"),
    )
    .expect("valid Via")
    .header(HeaderName::To, Bytes::from_static(b"<sip:callee.example>"))
    .expect("valid To")
    .header(
        HeaderName::From,
        Bytes::from_static(b"<sip:caller@example.net>;tag=caller"),
    )
    .expect("valid From")
    .header(
        HeaderName::CallId,
        Bytes::from_static(b"public-harness@example.net"),
    )
    .expect("valid Call-ID")
    .cseq(1, &Method::Invite)
    .expect("valid CSeq")
    .max_forwards(70)
    .build()
}

#[test]
fn a_downstream_test_places_and_answers_a_call_without_a_socket() {
    let mut call = CallHarness::perfect();

    call.place(invite()).expect("INVITE has a transaction key");
    assert_eq!(
        call.invitation().map(|request| &request.method),
        Some(&Method::Invite)
    );

    call.answer(StatusCode::new(200).expect("valid status"), "OK")
        .expect("the pending invitation can be answered");

    assert_eq!(
        call.response().map(|response| response.status.code()),
        Some(200)
    );
    assert_eq!(call.now().millis(), 0, "no wall-clock wait was needed");
}
