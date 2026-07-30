//! Attended transfer: `Replaces`, and the hijack it would be without the tags.
//!
//! The security case is the reason this story exists at all. A `Call-ID` is carried in every
//! message of a dialog and is visible to every element on the path — a proxy, a load balancer,
//! anything that logged a header. If a `Replaces` naming only the `Call-ID` were honoured,
//! anyone who had seen one message of a call could ask to be put in the middle of it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
#![allow(clippy::similar_names)]

use std::net::IpAddr;
use std::time::Duration;

use bytes::Bytes;
use sipx_call::transfer::Replaces;
use sipx_call::{Call, DialOptions, answer, answer_replacing, dial};
use sipx_sip::build::RequestBuilder;
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

/// Connect a caller and a callee, and hand back both sides.
async fn connect(
    caller_endpoint: &Handle,
    callee_endpoint: Handle,
    callee_incoming: &mut Receiver<Incoming>,
    from: &str,
) -> (Call, Call) {
    let callee_addr = callee_endpoint.local_addr();
    let to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
    let options = DialOptions::new(from, loopback());

    let answering = async {
        let incoming = callee_incoming.recv().await.expect("an INVITE arrives");
        answer(&callee_endpoint, &incoming, loopback())
            .await
            .expect("answers")
    };
    let dialling = dial(caller_endpoint, Target::udp(callee_addr), &to, &options);

    let (callee, caller) = tokio::join!(answering, dialling);
    (caller.expect("the call connects"), callee)
}

/// Wait for an INVITE, feeding anything else that arrives to the call it belongs to.
///
/// The interesting request is never the first one: a callee's channel carries the ACK for its
/// own 200 before anything else. A test that took the first request would be answering the ACK.
async fn next_invite(call: &mut Call, incoming: &mut Receiver<Incoming>) -> Incoming {
    let found = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(request) = incoming.recv().await {
            if request.request.method == Method::Invite
                && request.request.headers.get(&HeaderName::Replaces).is_some()
            {
                return request;
            }
            if request.request.method == Method::Invite {
                return request;
            }
            let _ = call.handle(&request).await;
        }
        panic!("the channel closed before an INVITE arrived");
    })
    .await;
    found.expect("an INVITE arrives")
}

/// Build an INVITE carrying a `Replaces` header, by hand.
fn invite_replacing(replaces: &str, to: &Uri, from_tag: &str) -> sipx_sip::Request {
    RequestBuilder::new(Method::Invite, to.clone())
        .header(HeaderName::To, Bytes::from(format!("<{to}>")))
        .expect("valid")
        .header(
            HeaderName::From,
            Bytes::from(format!("<sip:attacker@example.net>;tag={from_tag}")),
        )
        .expect("valid")
        .header(
            HeaderName::CallId,
            Bytes::from(format!("hijack-{from_tag}@example.net")),
        )
        .expect("valid")
        .cseq(1, &Method::Invite)
        .expect("valid")
        .header(HeaderName::Replaces, Bytes::from(replaces.to_owned()))
        .expect("valid")
        .header(HeaderName::Contact, "<sip:attacker@127.0.0.1:1>")
        .expect("valid")
        .header(HeaderName::ContentType, "application/sdp")
        .expect("valid")
        .max_forwards(70)
        .body(Bytes::from(
            "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\n\
             t=0 0\r\nm=audio 40000 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\n"
                .to_owned(),
        ))
        .build()
}

/// S-10's exit criterion, and the whole reason `Replaces` matches on three fields.
///
/// The attacker has the `Call-ID` — the easiest of the three to come by — and guesses the tags.
#[tokio::test]
async fn a_replaces_naming_someone_elses_dialog_is_refused() {
    let (alice_endpoint, _alice_unused) = endpoint().await;
    let (bob_endpoint, mut bob_incoming) = endpoint().await;
    let (attacker, _attacker_incoming) = endpoint().await;

    let (_alice, mut bob) = connect(
        &alice_endpoint,
        bob_endpoint.clone(),
        &mut bob_incoming,
        "<sip:alice@example.net>",
    )
    .await;

    let call_id = String::from_utf8_lossy(&bob.dialog.id.call_id).into_owned();
    let to = Uri::sip(Host::Name(HostName::new("bob.example").expect("valid")));

    // Every way of getting it wrong, including the one that matters: the right Call-ID with
    // guessed tags.
    let attempts = [
        format!("{call_id};to-tag=guess;from-tag=guess"),
        format!(
            "{call_id};to-tag={};from-tag=guess",
            tag(&bob.dialog.id.local_tag)
        ),
        format!(
            "{call_id};to-tag=guess;from-tag={}",
            tag(&bob.dialog.id.remote_tag)
        ),
        // The tags the right way round for the *wrong* orientation.
        format!(
            "{call_id};to-tag={};from-tag={}",
            tag(&bob.dialog.id.remote_tag),
            tag(&bob.dialog.id.local_tag)
        ),
    ];

    for (index, attempt) in attempts.iter().enumerate() {
        let invite = invite_replacing(attempt, &to, &format!("a{index}"));
        let mut responses = attacker
            .send(invite, Target::udp(bob_endpoint.local_addr()))
            .await
            .expect("sends");

        let incoming = next_invite(&mut bob, &mut bob_incoming).await;

        let outcome = answer_replacing(&bob_endpoint, &incoming, loopback(), &mut bob).await;
        assert!(
            outcome.is_err(),
            "attempt {index} must be refused: {attempt}"
        );

        let response = tokio::time::timeout(Duration::from_secs(5), responses.final_response())
            .await
            .expect("no timeout")
            .expect("a final response");
        assert_eq!(
            response.status.code(),
            481,
            "the refusal must give nothing away about which field was wrong"
        );

        // And the call it tried to displace is untouched.
        assert!(
            !bob.is_ended(),
            "attempt {index} ended a call it should not have"
        );
    }
}

/// A `Replaces` that names the dialog correctly *is* honoured — otherwise the test above would
/// pass against an implementation that refused everything.
#[tokio::test]
async fn a_replaces_naming_this_dialog_takes_the_call_over() {
    let (alice_endpoint, mut alice_incoming) = endpoint().await;
    let (bob_endpoint, mut bob_incoming) = endpoint().await;
    let (carol, mut carol_incoming) = endpoint().await;

    let (mut alice, mut bob) = connect(
        &alice_endpoint,
        bob_endpoint.clone(),
        &mut bob_incoming,
        "<sip:alice@example.net>",
    )
    .await;

    // Carol builds the header the way the transferor would: from *Bob's* point of view, whose
    // local tag is Bob's and whose remote tag is Alice's.
    let replaces = Replaces {
        call_id: bob.dialog.id.call_id.clone(),
        to_tag: bob.dialog.id.local_tag.clone(),
        from_tag: bob.dialog.id.remote_tag.clone(),
        early_only: false,
    };
    let to = Uri::sip(Host::Name(HostName::new("bob.example").expect("valid")));
    let invite = invite_replacing(&replaces.to_header(), &to, "carol");

    let mut responses = carol
        .send(invite, Target::udp(bob_endpoint.local_addr()))
        .await
        .expect("sends");
    let incoming = next_invite(&mut bob, &mut bob_incoming).await;

    let taken_over = answer_replacing(&bob_endpoint, &incoming, loopback(), &mut bob)
        .await
        .expect("a Replaces naming this dialog is honoured");

    let response = tokio::time::timeout(Duration::from_secs(5), responses.final_response())
        .await
        .expect("no timeout")
        .expect("a final response");
    assert_eq!(response.status.code(), 200);

    // The replaced call is over, and Alice is told so with a BYE rather than simply dropped.
    assert!(bob.is_ended(), "the replaced dialog must be terminated");
    let bye = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(request) = alice_incoming.recv().await {
            if request.request.method == Method::Bye {
                assert!(
                    alice.handle(&request).await.expect("handles"),
                    "the BYE must belong to Alice's call"
                );
                return;
            }
        }
        panic!("Alice was never told the call had ended");
    })
    .await;
    assert!(bye.is_ok(), "Alice must be sent a BYE, not simply dropped");
    assert!(alice.is_ended(), "and must act on it");

    // And the media of the replaced call has stopped.
    let before = bob.media().packets_sent();
    bob.media().play(&vec![0i16; 1600], 160).await;
    // A definition of silence: how long a hole has to be before "it stopped sending" is true.
    // The assertion below is negative, so load lengthens the window and can only make it fail —
    // there is no arrival to poll for, and waiting for a packet that must never come would be a
    // ten-second sleep in every run (`X-44`).
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        bob.media().packets_sent(),
        before,
        "a replaced call must stop sending"
    );

    drop(taken_over);
    let _ = carol_incoming.try_recv();
}

/// An INVITE with no `Replaces` at all is a request to *start* a call, not to replace one, and
/// answering it by tearing down an existing call would be catastrophic.
#[tokio::test]
async fn an_invite_without_replaces_does_not_displace_anything() {
    let (alice_endpoint, _alice_unused) = endpoint().await;
    let (bob_endpoint, mut bob_incoming) = endpoint().await;
    let (carol, _carol_incoming) = endpoint().await;

    let (_alice, mut bob) = connect(
        &alice_endpoint,
        bob_endpoint.clone(),
        &mut bob_incoming,
        "<sip:alice@example.net>",
    )
    .await;

    let to = Uri::sip(Host::Name(HostName::new("bob.example").expect("valid")));
    let invite = RequestBuilder::new(Method::Invite, to.clone())
        .header(HeaderName::To, Bytes::from(format!("<{to}>")))
        .expect("valid")
        .header(HeaderName::From, "<sip:carol@example.net>;tag=c1")
        .expect("valid")
        .header(HeaderName::CallId, Bytes::from_static(b"plain@example.net"))
        .expect("valid")
        .cseq(1, &Method::Invite)
        .expect("valid")
        .header(HeaderName::Contact, "<sip:carol@127.0.0.1:1>")
        .expect("valid")
        .max_forwards(70)
        .build();

    let mut responses = carol
        .send(invite, Target::udp(bob_endpoint.local_addr()))
        .await
        .expect("sends");
    let incoming = next_invite(&mut bob, &mut bob_incoming).await;

    assert!(
        answer_replacing(&bob_endpoint, &incoming, loopback(), &mut bob)
            .await
            .is_err()
    );
    let response = tokio::time::timeout(Duration::from_secs(5), responses.final_response())
        .await
        .expect("no timeout")
        .expect("a final response");
    assert_eq!(response.status.code(), 400);
    assert!(!bob.is_ended(), "an existing call must survive");
}

fn tag(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).into_owned()
}
