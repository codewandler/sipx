//! SDP-free confirmed dialogs for bounded signalling workloads (P-15).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use bytes::Bytes;
use sipx_call::{Dispatched, Dispatcher, SignallingEvent};
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::{HeaderName, Host, HostName, Method, StatusCode, Uri};
use sipx_transport::{Config, Handle, Incoming, Target, TransportKind, bind};
use tokio::sync::mpsc;

async fn endpoint() -> (Handle, mpsc::Receiver<Incoming>) {
    bind(Config::new("127.0.0.1:0".parse().expect("valid address")))
        .await
        .expect("endpoint binds")
}

fn uri() -> Uri {
    Uri::sip(Host::Name(
        HostName::new("load.invalid").expect("valid host"),
    ))
}

fn request(
    peer: &Handle,
    method: &Method,
    call_id: &str,
    to_tag: Option<&str>,
    cseq: u32,
) -> sipx_sip::Request {
    let to = to_tag.map_or_else(
        || "<sip:load@load.invalid>".to_owned(),
        |tag| format!("<sip:load@load.invalid>;tag={tag}"),
    );
    RequestBuilder::new(method.clone(), uri())
        .header(
            HeaderName::Via,
            Bytes::from(format!(
                "SIP/2.0/UDP {};rport;branch={}",
                peer.sent_by_for(TransportKind::Udp),
                sipx_transport::new_branch()
            )),
        )
        .expect("via")
        .header(HeaderName::To, Bytes::from(to))
        .expect("to")
        .header(
            HeaderName::From,
            Bytes::from_static(b"<sip:driver@driver.invalid>;tag=f-fixed"),
        )
        .expect("from")
        .header(HeaderName::CallId, Bytes::from(call_id.to_owned()))
        .expect("call-id")
        .cseq(cseq, method)
        .expect("cseq")
        .header(
            HeaderName::Contact,
            Bytes::from(format!("<sip:driver@{}>", peer.local_addr())),
        )
        .expect("contact")
        .max_forwards(70)
        .build()
}

#[tokio::test(start_paused = true)]
async fn bodyless_invite_ack_bye_is_a_complete_signalling_dialog() {
    let (callee, incoming) = endpoint().await;
    let callee_addr = callee.local_addr();
    let mut dispatcher = Dispatcher::new(callee.clone(), incoming);
    let calls = dispatcher.calls();
    let (surfaced, mut invitations) = mpsc::channel(1);
    let pump = tokio::spawn(async move {
        while let Some(item) = dispatcher.next().await {
            if surfaced.send(item).await.is_err() {
                return;
            }
        }
    });
    let (peer, _peer_incoming) = endpoint().await;
    let invite = request(&peer, &Method::Invite, "signal-1@driver.invalid", None, 1);
    let asking = {
        let peer = peer.clone();
        tokio::spawn(async move {
            let mut responses = peer
                .send(invite, Target::udp(callee_addr))
                .await
                .expect("INVITE sends");
            let first = responses.next().await.expect("100 Trying");
            let sipx_sip::transaction::TuEvent::Response(first) = first else {
                panic!("first transaction event was not a response");
            };
            assert_eq!(first.status.code(), 100);
            responses
                .final_response()
                .await
                .expect("final INVITE response")
        })
    };

    let invitation = match invitations.recv().await.expect("surfaced invitation") {
        Dispatched::Invitation(invitation) => invitation,
        other => panic!("expected invitation, got {other:?}"),
    };
    invitation.trying(&callee).await.expect("sends Trying");
    let contact = format!("<sip:load@{}>", callee.advertised());
    let mut call = invitation
        .answer_signalling_with_tag(&callee, contact, "t-fixed")
        .await
        .expect("answers without SDP");
    let accepted = asking.await.expect("INVITE task joins");
    assert_eq!(accepted.status.code(), 200);
    assert!(accepted.body().is_empty());
    assert_eq!(
        accepted.headers.value(&HeaderName::To).as_deref(),
        Some(b"<sip:load@load.invalid>;tag=t-fixed".as_slice())
    );

    peer.send_directly(
        request(
            &peer,
            &Method::Ack,
            "signal-1@driver.invalid",
            Some("t-fixed"),
            1,
        ),
        Target::udp(callee_addr),
    )
    .await
    .expect("ACK sends directly");
    assert_eq!(
        call.next().await.expect("ACK event"),
        SignallingEvent::Acknowledged
    );

    let bye = request(
        &peer,
        &Method::Bye,
        "signal-1@driver.invalid",
        Some("t-fixed"),
        2,
    );
    let ending = {
        let peer = peer.clone();
        tokio::spawn(async move {
            let mut responses = peer
                .send(bye, Target::udp(callee_addr))
                .await
                .expect("BYE sends");
            responses.final_response().await.expect("BYE response")
        })
    };
    assert_eq!(
        call.next().await.expect("BYE event"),
        SignallingEvent::RemoteBye
    );
    assert_eq!(ending.await.expect("BYE task joins").status.code(), 200);
    assert!(call.is_ended());

    calls.forget(call.dialog());
    drop(call);
    assert!(calls.is_empty());
    pump.abort();
    let _ = pump.await;
    callee.shutdown().await;
    peer.shutdown().await;
}

#[tokio::test]
async fn wrong_ack_sequence_is_typed_and_does_not_establish() {
    let (callee, incoming) = endpoint().await;
    let callee_addr = callee.local_addr();
    let mut dispatcher = Dispatcher::new(callee.clone(), incoming);
    let (surfaced, mut invitations) = mpsc::channel(1);
    let pump = tokio::spawn(async move {
        while let Some(item) = dispatcher.next().await {
            if surfaced.send(item).await.is_err() {
                return;
            }
        }
    });
    let (peer, _peer_incoming) = endpoint().await;
    let invite = request(&peer, &Method::Invite, "signal-2@driver.invalid", None, 1);
    let asking = {
        let peer = peer.clone();
        tokio::spawn(async move {
            let mut responses = peer
                .send(invite, Target::udp(callee_addr))
                .await
                .expect("INVITE sends");
            responses.final_response().await.expect("accepted")
        })
    };
    let invitation = match invitations.recv().await.expect("surfaced") {
        Dispatched::Invitation(invitation) => invitation,
        other => panic!("expected invitation, got {other:?}"),
    };
    let mut call = invitation
        .answer_signalling_with_tag(
            &callee,
            format!("<sip:load@{}>", callee.advertised()),
            "t-fixed",
        )
        .await
        .expect("answers");
    assert_eq!(asking.await.expect("joins").status.code(), 200);
    peer.send_directly(
        request(
            &peer,
            &Method::Ack,
            "signal-2@driver.invalid",
            Some("t-fixed"),
            9,
        ),
        Target::udp(callee_addr),
    )
    .await
    .expect("bad ACK sends");
    assert_eq!(
        call.next().await.expect("invalid ACK event"),
        SignallingEvent::InvalidAck
    );
    assert!(!call.is_acknowledged());
    tokio::time::pause();
    // Advancing the clock asks whether Timer H's specified failure bound produces its typed event.
    tokio::time::advance(std::time::Duration::from_secs(32)).await;
    assert_eq!(
        call.next().await.expect("Timer H event"),
        SignallingEvent::AckTimedOut
    );
    assert!(call.is_ended());
    pump.abort();
    let _ = pump.await;
    callee.shutdown().await;
    peer.shutdown().await;
}

#[tokio::test]
async fn bye_response_must_match_the_confirmed_dialog() {
    let (callee, incoming) = endpoint().await;
    let callee_addr = callee.local_addr();
    let mut dispatcher = Dispatcher::new(callee.clone(), incoming);
    let (surfaced, mut invitations) = mpsc::channel(1);
    let pump = tokio::spawn(async move {
        while let Some(item) = dispatcher.next().await {
            if surfaced.send(item).await.is_err() {
                return;
            }
        }
    });
    let (peer, mut peer_incoming) = endpoint().await;
    let invite = request(&peer, &Method::Invite, "signal-3@driver.invalid", None, 1);
    let asking = {
        let peer = peer.clone();
        tokio::spawn(async move {
            let mut responses = peer
                .send(invite, Target::udp(callee_addr))
                .await
                .expect("INVITE sends");
            responses.final_response().await.expect("accepted")
        })
    };
    let invitation = match invitations.recv().await.expect("surfaced") {
        Dispatched::Invitation(invitation) => invitation,
        other => panic!("expected invitation, got {other:?}"),
    };
    let mut call = invitation
        .answer_signalling_with_tag(
            &callee,
            format!("<sip:load@{}>", callee.advertised()),
            "t-fixed",
        )
        .await
        .expect("answers");
    assert_eq!(asking.await.expect("joins").status.code(), 200);
    peer.send_directly(
        request(
            &peer,
            &Method::Ack,
            "signal-3@driver.invalid",
            Some("t-fixed"),
            1,
        ),
        Target::udp(callee_addr),
    )
    .await
    .expect("ACK sends");
    assert_eq!(
        call.next().await.expect("ACK event"),
        SignallingEvent::Acknowledged
    );

    let hanging_up = tokio::spawn(async move {
        let result = call.hang_up(std::time::Duration::from_secs(2)).await;
        (call, result)
    });
    let bye = peer_incoming.recv().await.expect("BYE arrives");
    assert_eq!(bye.request.method, Method::Bye);
    let response = ResponseBuilder::to_request(
        &bye.request,
        StatusCode::new(200).expect("valid status"),
        "OK",
    )
    .expect("response builder")
    .set_header(
        &HeaderName::CallId,
        Bytes::from_static(b"wrong-dialog@driver.invalid"),
    )
    .expect("wrong Call-ID header")
    .build();
    peer.respond(&bye.key, response)
        .await
        .expect("malicious response sends");
    let (call, result) = hanging_up.await.expect("hang-up task joins");
    assert!(matches!(
        result,
        Err(sipx_call::Error::InvalidDialogResponse)
    ));
    assert!(call.is_ended());

    pump.abort();
    let _ = pump.await;
    callee.shutdown().await;
    peer.shutdown().await;
}
