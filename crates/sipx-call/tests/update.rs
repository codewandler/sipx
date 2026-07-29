//! The UPDATE method (RFC 3311), on the wire.
//!
//! What UPDATE is for is the case a re-INVITE cannot reach: a session that has been described
//! but not yet answered. Until the INVITE has a final response there is a transaction in
//! progress and a second INVITE inside it is not a thing SIP has, so before this method the
//! only way to change an early session was to tear the call attempt down and place it again.
//!
//! The peer here is built from raw messages rather than from a second sipx, because the point
//! of most of these is what sipx puts on the wire when the *peer* does something ill-timed, and
//! sipx's own side would never do those things.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::similar_names)]

use std::net::IpAddr;
use std::time::Duration;

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

fn via(endpoint: &Handle) -> bytes::Bytes {
    bytes::Bytes::from(format!(
        "SIP/2.0/UDP {};rport;branch={}",
        endpoint.sent_by_for(sipx_transport::TransportKind::Udp),
        sipx_transport::new_branch()
    ))
}

/// An offer naming one payload type, so that a renegotiation is visible as a codec change.
fn sdp(port: u16, payload: u8, encoding: &str) -> String {
    format!(
        "v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
         m=audio {port} RTP/AVP {payload}\r\na=rtpmap:{payload} {encoding}/8000\r\na=sendrecv\r\n"
    )
}

/// The INVITE the peer sends: an offer, 100rel on offer, and an `Allow` that lists UPDATE.
fn raw_invite(endpoint: &Handle, call_id: &'static str, body: &str) -> Request {
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
        .header(HeaderName::Supported, bytes::Bytes::from_static(b"100rel"))
        .expect("supported")
        .header(
            HeaderName::Allow,
            bytes::Bytes::from_static(b"INVITE, ACK, CANCEL, BYE, PRACK, UPDATE"),
        )
        .expect("allow")
        .header(
            HeaderName::Contact,
            bytes::Bytes::from_static(b"<sip:caller@127.0.0.1>"),
        )
        .expect("contact")
        .header(
            HeaderName::ContentType,
            bytes::Bytes::from_static(b"application/sdp"),
        )
        .expect("content-type")
        .max_forwards(70)
        .body(bytes::Bytes::from(body.to_owned()))
        .build()
}

/// An in-dialog request from the peer, inside the dialog `tag` established.
fn raw_in_dialog(
    endpoint: &Handle,
    method: &Method,
    call_id: &'static str,
    tag: &str,
    cseq: u32,
    body: Option<&str>,
) -> Request {
    let mut builder = sipx_sip::build::RequestBuilder::new(method.clone(), to_uri())
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
        .cseq(cseq, method)
        .expect("cseq")
        .header(
            HeaderName::Contact,
            bytes::Bytes::from_static(b"<sip:caller@127.0.0.1>"),
        )
        .expect("contact")
        .max_forwards(70);
    if let Some(body) = body {
        builder = builder
            .header(
                HeaderName::ContentType,
                bytes::Bytes::from_static(b"application/sdp"),
            )
            .expect("content-type")
            .body(bytes::Bytes::from(body.to_owned()));
    }
    builder.build()
}

/// The story's failing-first test.
///
/// A session is described in the INVITE, answered in a reliable provisional, and then *changed*
/// — all before the invitation has a final response. Without RFC 3311 the only way to reach the
/// same place is to abandon the call attempt and place it again.
#[tokio::test]
async fn an_update_renegotiates_a_session_before_it_is_answered() {
    const CALL_ID: &str = "update-1@sipx";
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, _caller_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    // PCMU in the INVITE; PCMA in the UPDATE. The codec is what makes the renegotiation
    // visible on the call that eventually results.
    let mut responses = caller_endpoint
        .send(
            raw_invite(&caller_endpoint, CALL_ID, &sdp(40000, 0, "PCMU")),
            Target::udp(callee_addr),
        )
        .await
        .expect("sends");

    let invite = callee_incoming.recv().await.expect("the INVITE arrives");
    // The answer travels in the provisional, which is the only place RFC 3262 §5 allows one
    // before the 200 — and without it RFC 3311 §5.1 would forbid the UPDATE's offer outright,
    // because this side would still owe an answer.
    let mut ringing = sipx_call::ring_early(
        &callee_endpoint,
        &invite,
        183,
        "Session Progress",
        loopback(),
    )
    .await
    .expect("rings with an answer");
    let tag = ringing.tag().to_owned();
    assert!(ringing.has_early_session());

    let provisional = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(sipx_sip::transaction::TuEvent::Response(response)) = responses.next().await
                && !response.status.is_final()
                && response.status.code() > 100
            {
                return *response;
            }
        }
    })
    .await
    .expect("a provisional establishes the early dialog");
    assert!(provisional.headers.value(&HeaderName::Contact).is_some());
    let early_answer = String::from_utf8_lossy(provisional.body()).into_owned();
    assert!(
        early_answer.contains("m=audio"),
        "the provisional carried no answer, so the session was never negotiated: {early_answer:?}"
    );
    // RFC 3311 §4: this is where the caller is told it may renegotiate at all.
    let allow = provisional
        .headers
        .value(&HeaderName::Allow)
        .expect("a reliable provisional carrying SDP must say what it allows");
    assert!(
        String::from_utf8_lossy(&allow).contains("UPDATE"),
        "the provisional did not advertise UPDATE, so no compliant peer would send one"
    );

    // The renegotiation, inside the early dialog.
    let update = raw_in_dialog(
        &caller_endpoint,
        &Method::Update,
        CALL_ID,
        &tag,
        2,
        Some(&sdp(40002, 8, "PCMA")),
    );
    let mut update_responses = caller_endpoint
        .send(update, Target::udp(callee_addr))
        .await
        .expect("sends the UPDATE");

    let arrived = tokio::time::timeout(Duration::from_secs(2), callee_incoming.recv())
        .await
        .expect("the UPDATE arrives")
        .expect("a request");
    assert_eq!(arrived.request.method, Method::Update);
    assert!(
        ringing.on_update(&arrived).await.expect("handled"),
        "the UPDATE was not recognised as belonging to the early dialog"
    );

    let answered = tokio::time::timeout(Duration::from_secs(2), update_responses.final_response())
        .await
        .expect("the UPDATE is answered")
        .expect("a response");
    assert_eq!(
        answered.status.code(),
        200,
        "an early-dialog renegotiation was refused: {} {}",
        answered.status.code(),
        String::from_utf8_lossy(&answered.reason)
    );
    let body = String::from_utf8_lossy(answered.body()).into_owned();
    assert!(
        body.contains("RTP/AVP 8"),
        "the 2xx to the UPDATE carried no answer to the new offer: {body:?}"
    );

    // And the renegotiation is what the call is built on when the invitation is finally
    // accepted. Asserting the codec rather than merely that a 200 came back is the difference
    // between this test and one that would also pass if the answer were built and thrown away.
    let call = sipx_call::answer_early(&callee_endpoint, &invite, &mut ringing)
        .await
        .expect("answers");
    assert_eq!(
        call.media().codec(),
        sipx_media::Codec::Pcma,
        "the call was answered on the codec the INVITE offered, not the one the UPDATE settled"
    );
    assert!(!call.is_ended());
}
