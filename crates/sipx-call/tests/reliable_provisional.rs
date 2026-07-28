//! Reliable provisional responses (RFC 3262), end to end.
//!
//! An ordinary `180 Ringing` is fire-and-forget, and over UDP it is sometimes simply lost. The
//! caller then hears nothing while the callee's phone rings — and if the provisional carried
//! early media, the call connects to silence. Some carriers will not accept a call at all
//! without 100rel, which is the practical reason any of this is here.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::similar_names)]

use std::net::IpAddr;
use std::time::Duration;

use sipx_call::{Call, DialOptions, Error, answer_ringing, dial, ring};
use sipx_sip::rel::{RAck, RSeq};
use sipx_sip::{HeaderName, Host, HostName, Method, Request, Uri};
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

fn options() -> DialOptions {
    DialOptions::new("<sip:caller@example.net>", loopback())
}

fn via(endpoint: &Handle) -> bytes::Bytes {
    bytes::Bytes::from(format!(
        "SIP/2.0/UDP {};rport;branch={}",
        endpoint.sent_by_for(sipx_transport::TransportKind::Udp),
        sipx_transport::new_branch()
    ))
}

/// An INVITE built by hand, so the test controls exactly what the caller claims about 100rel.
///
/// `advertisement` is the header naming the option tag — `Supported` for a caller that can do
/// 100rel, `Require` for one that insists.
fn raw_invite(endpoint: &Handle, call_id: &'static str, advertisement: &HeaderName) -> Request {
    sipx_sip::build::RequestBuilder::new(Method::Invite, to_uri())
        .header(HeaderName::Via, via(endpoint))
        .expect("via")
        .header(
            HeaderName::To,
            bytes::Bytes::from_static(b"<sip:callee.example>"),
        )
        .expect("to")
        .header(
            HeaderName::From,
            bytes::Bytes::from_static(b"<sip:caller@example.net>;tag=abc"),
        )
        .expect("from")
        .header(
            HeaderName::CallId,
            bytes::Bytes::from_static(call_id.as_bytes()),
        )
        .expect("call-id")
        .cseq(1, &Method::Invite)
        .expect("cseq")
        .header(advertisement.clone(), bytes::Bytes::from_static(b"100rel"))
        .expect("the option tag")
        .header(
            HeaderName::Contact,
            bytes::Bytes::from_static(b"<sip:caller@127.0.0.1>"),
        )
        .expect("contact")
        .max_forwards(70)
        .build()
}

/// The PRACK acknowledging `rseq`, inside the dialog `tag` established.
fn raw_prack(endpoint: &Handle, call_id: &'static str, tag: &str, rseq: u32) -> Request {
    sipx_sip::build::RequestBuilder::new(Method::Prack, to_uri())
        .header(HeaderName::Via, via(endpoint))
        .expect("via")
        .header(
            HeaderName::To,
            bytes::Bytes::from(format!("<sip:callee.example>;tag={tag}")),
        )
        .expect("to")
        .header(
            HeaderName::From,
            bytes::Bytes::from_static(b"<sip:caller@example.net>;tag=abc"),
        )
        .expect("from")
        .header(
            HeaderName::CallId,
            bytes::Bytes::from_static(call_id.as_bytes()),
        )
        .expect("call-id")
        .cseq(2, &Method::Prack)
        .expect("cseq")
        .header(
            HeaderName::RAck,
            bytes::Bytes::from(format!("{rseq} 1 INVITE")),
        )
        .expect("rack")
        .max_forwards(70)
        .build()
}

/// Read reliable provisionals off a transaction until `wanted` have arrived or time runs out.
async fn collect_provisionals(
    responses: &mut sipx_transport::Responses,
    wanted: usize,
    within: Duration,
) -> Vec<u32> {
    let mut seen: Vec<u32> = Vec::new();
    let _ = tokio::time::timeout(within, async {
        while seen.len() < wanted {
            match responses.next().await {
                Some(sipx_sip::transaction::TuEvent::Response(response)) => {
                    if let Some(Ok(rseq)) = response.headers.typed::<RSeq>() {
                        assert_eq!(response.status.code(), 180);
                        let require = response
                            .headers
                            .value(&HeaderName::Require)
                            .expect("Require is present on a reliable provisional");
                        assert!(String::from_utf8_lossy(&require).contains("100rel"));
                        seen.push(rseq.0);
                    }
                }
                Some(_) => {}
                None => break,
            }
        }
    })
    .await;
    seen
}

/// The story's failing-first test.
///
/// Driven from the raw messages rather than through `dial`, because the point is what the
/// *answering* side puts on the wire and keeps putting there: sipx's own caller PRACKs the
/// first one immediately, which is correct behaviour and would hide the retransmission
/// entirely.
#[tokio::test]
async fn a_reliable_provisional_is_retransmitted_until_pracked() {
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, caller_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    // A caller that says it supports 100rel and then says nothing more. RFC 3262 §3 has the
    // UAS retransmit at T1, doubling, until a PRACK arrives or 64*T1 has passed.
    let mut responses = caller_endpoint
        .send(
            raw_invite(&caller_endpoint, "rel-1@sipx", &HeaderName::Supported),
            Target::udp(callee_addr),
        )
        .await
        .expect("sends");

    let incoming = callee_incoming.recv().await.expect("the INVITE arrives");
    let ringing = ring(&callee_endpoint, &incoming, 180, "Ringing", true)
        .await
        .expect("rings");
    assert!(
        ringing.is_reliable(),
        "the provisional was not sent reliably"
    );
    assert!(!ringing.is_acknowledged());

    // What actually arrives. The first is the original; anything after it is a retransmission,
    // which is the behaviour under test.
    let seen = collect_provisionals(&mut responses, 3, Duration::from_secs(3)).await;

    let (&rseq, rest) = seen
        .split_first()
        .expect("a reliable provisional was never sent at all");
    assert!(
        !rest.is_empty(),
        "a reliable provisional was sent once and never repeated: {seen:?}"
    );
    assert!(
        rest.iter().all(|&later| later == rseq),
        "the retransmissions changed the sequence number: {seen:?}"
    );
    assert!(
        (1..=sipx_sip::rel::MAX_FIRST_RSEQ).contains(&rseq),
        "the first RSeq is outside the range RFC 3262 §3 allows: {rseq}"
    );

    // Now acknowledge it, and the retransmissions must stop.
    let mut ringing = ringing;
    let prack = raw_prack(&caller_endpoint, "rel-1@sipx", ringing.tag(), rseq);
    let _ = caller_endpoint
        .send(prack, Target::udp(callee_addr))
        .await
        .expect("sends the PRACK");

    let prack_in = tokio::time::timeout(Duration::from_secs(2), callee_incoming.recv())
        .await
        .expect("the PRACK arrives")
        .expect("a request");
    assert!(
        ringing.on_prack(&prack_in).await.expect("handled"),
        "the PRACK was not recognised as acknowledging the provisional"
    );
    assert!(ringing.is_acknowledged());

    // Nothing further, for longer than the next interval would have been.
    let after = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match responses.next().await {
                Some(sipx_sip::transaction::TuEvent::Response(response)) => {
                    if response.headers.typed::<RSeq>().is_some() {
                        return true;
                    }
                }
                Some(_) => {}
                None => return false,
            }
        }
    })
    .await;
    assert!(
        after.is_err() || after == Ok(false),
        "the provisional was still being retransmitted after it was acknowledged"
    );
    drop(caller_incoming);
}

#[tokio::test]
async fn a_caller_that_requires_100rel_is_refused_when_it_is_off() {
    // RFC 3262 §3: "If the UAS is unwilling to do so, it MUST reject the initial request with a
    // 420 (Bad Extension) and include an Unsupported header field containing the option tag."
    // Refusing plainly is the point — a caller left waiting for an RSeq that never comes cannot
    // tell that from a dead network.
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, _caller_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    let refusing = tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("an INVITE");
        // 100rel switched off locally.
        ring(&callee_endpoint, &incoming, 180, "Ringing", false).await
    });

    let mut responses = caller_endpoint
        .send(
            raw_invite(&caller_endpoint, "rel-2@sipx", &HeaderName::Require),
            Target::udp(callee_addr),
        )
        .await
        .expect("sends");
    let response = tokio::time::timeout(Duration::from_secs(3), responses.final_response())
        .await
        .expect("a final response arrives")
        .expect("a response");

    assert_eq!(response.status.code(), 420);
    let unsupported = response
        .headers
        .value(&HeaderName::Unsupported)
        .expect("Unsupported names the tag it could not do");
    assert!(String::from_utf8_lossy(&unsupported).contains("100rel"));

    let outcome = refusing.await.expect("the answering side finishes");
    assert!(matches!(outcome, Err(Error::Rejected { status: 420, .. })));
}

#[tokio::test]
async fn the_caller_pracks_a_reliable_provisional_and_the_call_completes() {
    // Both halves in sipx, which is what proves the two agree: the UAS numbers and
    // retransmits, the UAC acknowledges in order, and the call still connects afterwards on
    // the *same* dialog the provisional established.
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, _caller_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    let answering = tokio::spawn(async move {
        let invite = callee_incoming.recv().await.expect("an INVITE");
        let mut ringing = ring(&callee_endpoint, &invite, 180, "Ringing", true)
            .await
            .expect("rings");
        assert!(ringing.is_reliable(), "the caller did not offer 100rel");

        // The PRACK sipx's own caller sends.
        let prack = tokio::time::timeout(Duration::from_secs(3), callee_incoming.recv())
            .await
            .expect("a PRACK arrives")
            .expect("a request");
        assert_eq!(prack.request.method, Method::Prack);
        let ack = prack
            .request
            .headers
            .typed::<RAck>()
            .expect("RAck is present")
            .expect("it parses");
        assert_eq!(ack.cseq, 1);
        assert_eq!(ack.method, b"INVITE");
        assert!(
            ringing.on_prack(&prack).await.expect("handled"),
            "the PRACK did not match: {ack:?}"
        );

        let call = answer_ringing(&callee_endpoint, &invite, loopback(), &ringing)
            .await
            .expect("answers");
        (call, ringing.tag().to_owned())
    });

    let caller: Call = dial(
        &caller_endpoint,
        Target::udp(callee_addr),
        &to_uri(),
        &options(),
    )
    .await
    .expect("the call connects");

    let (_callee, tag) = answering.await.expect("the answering side finishes");

    // The dialog the 200 confirmed is the one the provisional created. A fresh tag on the 200
    // would have made a second dialog: the caller ACKs the one it knows, the callee waits for
    // an ACK to the other, and the 200 is retransmitted for 32 seconds into a working call.
    assert_eq!(
        caller.dialog.id.remote_tag,
        tag.as_bytes(),
        "the 200 established a different dialog from the reliable provisional"
    );
    assert!(!caller.is_ended());
}

#[tokio::test]
async fn the_invite_offers_100rel() {
    // §3 forbids a UAS from sending a reliable provisional unless the request said it could.
    // A UAC that stays quiet therefore gets unreliable ringing even from a UAS that would
    // rather not send it.
    let (peer_endpoint, mut peer_incoming) = endpoint().await;
    let (caller_endpoint, _caller_incoming) = endpoint().await;
    let peer_addr = peer_endpoint.local_addr();

    let seen = tokio::spawn(async move { peer_incoming.recv().await.expect("an INVITE") });
    let _ = dial(
        &caller_endpoint,
        Target::udp(peer_addr),
        &to_uri(),
        &options().with_timeout(Duration::from_millis(300)),
    )
    .await;

    let invite = seen.await.expect("the INVITE arrives").request;
    let offered = sipx_sip::rel::Offered::in_request(&invite);
    assert!(offered.supported, "the INVITE does not offer 100rel");
    // Offered, not required: insisting would make every call fail against the many UASs that
    // do not implement RFC 3262 at all.
    assert!(!offered.required);
}
