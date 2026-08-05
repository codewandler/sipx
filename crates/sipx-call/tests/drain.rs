//! Bounded graceful call drain (T-29).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::similar_names
)]

use std::time::Duration;

use bytes::Bytes;
use sipx_call::{Dispatched, Dispatcher, SignallingEvent};
use sipx_sip::build::RequestBuilder;
use sipx_sip::{HeaderName, Host, HostName, Method, Uri};
use sipx_transport::{Target, TransportKind, in_process_pair};

fn uri() -> Uri {
    Uri::sip(Host::Name(
        HostName::new("drain.invalid").expect("valid host"),
    ))
}

fn request(
    method: &Method,
    call_id: &'static [u8],
    to_tag: Option<&str>,
    cseq: u32,
) -> sipx_sip::Request {
    let to = to_tag.map_or_else(
        || "<sip:callee@drain.invalid>".to_owned(),
        |tag| format!("<sip:callee@drain.invalid>;tag={tag}"),
    );
    let builder = RequestBuilder::new(method.clone(), uri())
        .header(
            HeaderName::Via,
            Bytes::from(format!(
                "SIP/2.0/TCP caller.invalid;branch=z9hG4bK{method}-{cseq}"
            )),
        )
        .expect("Via")
        .header(HeaderName::To, Bytes::from(to))
        .expect("To")
        .header(
            HeaderName::From,
            Bytes::from_static(b"<sip:caller@drain.invalid>;tag=caller"),
        )
        .expect("From")
        .header(HeaderName::CallId, Bytes::from_static(call_id))
        .expect("Call-ID")
        .cseq(cseq, method)
        .expect("CSeq")
        .header(
            HeaderName::Contact,
            Bytes::from_static(b"<sip:caller@drain.invalid>"),
        )
        .expect("Contact")
        .max_forwards(70);
    let builder = if method == &Method::Subscribe {
        builder
            .header(HeaderName::Event, Bytes::from_static(b"presence"))
            .expect("Event")
            .header(HeaderName::Expires, Bytes::from_static(b"60"))
            .expect("Expires")
    } else {
        builder
    };
    builder.build()
}

async fn establish(
    caller: &sipx_transport::Handle,
    callee: &sipx_transport::Handle,
    dispatcher: &mut Dispatcher,
) -> sipx_call::SignallingCall {
    let invite = request(&Method::Invite, b"existing@drain.invalid", None, 1);
    let target = Target::new(callee.local_addr(), TransportKind::Tcp);
    let asking = caller.send(invite, target).await.expect("INVITE is sent");
    let invitation = match dispatcher.next().await.expect("INVITE is surfaced") {
        Dispatched::Invitation(invitation) => invitation,
        other => panic!("expected invitation, got {other:?}"),
    };
    let call = invitation
        .answer_signalling_with_tag(
            callee,
            Bytes::from_static(b"<sip:callee@drain.invalid>"),
            "callee",
        )
        .await
        .expect("dialog is established");
    let mut asking = asking;
    let accepted = asking
        .final_response()
        .await
        .expect("INVITE receives final response");
    assert_eq!(accepted.status.code(), 200);

    caller
        .send_directly(
            request(&Method::Ack, b"existing@drain.invalid", Some("callee"), 1),
            Target::new(callee.local_addr(), TransportKind::Tcp),
        )
        .await
        .expect("ACK is sent");
    call
}

/// D1/D2: close new-call admission while preserving the live dialog until its BYE settles.
#[tokio::test]
async fn a_graceful_drain_refuses_new_calls_and_serves_an_existing_dialog() {
    let ((caller, _), (callee, incoming)) = in_process_pair(32).expect("runtime is entered");
    let mut dispatcher = Dispatcher::new(callee.clone(), incoming);
    let mut call = establish(&caller, &callee, &mut dispatcher).await;

    dispatcher.begin_drain();
    let draining = tokio::spawn(async move { dispatcher.drain(Duration::from_secs(30)).await });
    tokio::task::yield_now().await;
    assert!(
        !draining.is_finished(),
        "the live dialog, not a grace-period sleep, holds the drain open"
    );

    let mut refused = caller
        .send(
            request(&Method::Invite, b"new@drain.invalid", None, 1),
            Target::new(callee.local_addr(), TransportKind::Tcp),
        )
        .await
        .expect("the peer can attempt a new call");
    let refusal = refused
        .final_response()
        .await
        .expect("draining endpoint answers the attempt");
    assert_eq!(refusal.status.code(), 503);
    assert!(refusal.headers.value(&HeaderName::RetryAfter).is_some());

    let mut subscription = caller
        .send(
            request(&Method::Subscribe, b"subscription@drain.invalid", None, 1),
            Target::new(callee.local_addr(), TransportKind::Tcp),
        )
        .await
        .expect("the peer can attempt a new subscription dialog");
    assert_eq!(
        subscription
            .final_response()
            .await
            .expect("new subscription is explicitly refused")
            .status
            .code(),
        503
    );

    assert_eq!(
        call.next().await.expect("ACK is routed during drain"),
        SignallingEvent::Acknowledged
    );
    let mut bye = caller
        .send(
            request(&Method::Bye, b"existing@drain.invalid", Some("callee"), 2),
            Target::new(callee.local_addr(), TransportKind::Tcp),
        )
        .await
        .expect("in-dialog BYE is sent");
    assert_eq!(
        call.next().await.expect("BYE reaches the existing dialog"),
        SignallingEvent::RemoteBye
    );
    assert_eq!(
        bye.final_response()
            .await
            .expect("BYE transaction reaches final response")
            .status
            .code(),
        200
    );
    drop(call);

    let report = tokio::time::timeout(Duration::from_secs(5), draining)
        .await
        .expect("a bound on failure: terminal dialog and transaction release drain")
        .expect("drain task joins")
        .expect("drain observes endpoint state");
    assert!(report.completed);
    assert_eq!(report.terminated_dialogs, 0);
    assert_eq!(report.terminated_transactions, 0);
    assert_eq!(report.counts.draining, 2);
    caller.shutdown().await;
}

/// D4: a deadline is explicit forced termination, with both live dimensions preserved in output.
#[tokio::test]
async fn deadline_expiry_counts_and_closes_the_remaining_work() {
    let ((caller, _), (callee, incoming)) = in_process_pair(8).expect("runtime is entered");
    let mut dispatcher = Dispatcher::new(callee.clone(), incoming);
    let call = establish(&caller, &callee, &mut dispatcher).await;
    let _held = caller
        .send(
            request(
                &Method::Update,
                b"existing@drain.invalid",
                Some("callee"),
                2,
            ),
            Target::new(callee.local_addr(), TransportKind::Tcp),
        )
        .await
        .expect("an in-dialog transaction is live at the deadline");

    let report = dispatcher
        .drain(Duration::ZERO)
        .await
        .expect("deadline cleanup observes state");
    assert!(!report.completed);
    assert_eq!(report.terminated_dialogs, 1);
    assert!(report.terminated_transactions >= 1);
    assert_eq!(report.remaining.dialogs, 1);
    drop(call);
    caller.shutdown().await;
}
