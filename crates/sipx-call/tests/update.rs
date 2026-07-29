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

/// The peer's `Contact`, naming the port it actually listens on.
///
/// Not a bare host: an in-dialog request sipx originates goes to the remote target, and a
/// `Contact` without a port sends it to 5060 — where nothing in this test is listening.
fn contact(endpoint: &Handle) -> bytes::Bytes {
    bytes::Bytes::from(format!("<sip:caller@{}>", endpoint.local_addr()))
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
        .header(HeaderName::Contact, contact(endpoint))
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
        .header(HeaderName::Contact, contact(endpoint))
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

/// §5.2's third rule, on the wire: an offer arriving while this side owes one.
///
/// Rung the ordinary way, so the INVITE's offer has had no answer. **500, not 491** — nothing of
/// ours is outstanding, so this is not glare; the peer is early, and telling it otherwise would
/// send it into RFC 3261 §14.1's randomised back-off instead of the retry that will work.
#[tokio::test]
async fn an_offer_arriving_while_this_side_owes_an_answer_is_refused_500() {
    const CALL_ID: &str = "update-2@sipx";
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, _caller_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    let mut responses = caller_endpoint
        .send(
            raw_invite(&caller_endpoint, CALL_ID, &sdp(40000, 0, "PCMU")),
            Target::udp(callee_addr),
        )
        .await
        .expect("sends");

    let invite = callee_incoming.recv().await.expect("the INVITE arrives");
    let mut ringing = sipx_call::ring(&callee_endpoint, &invite, 180, "Ringing", true)
        .await
        .expect("rings");
    assert!(
        !ringing.has_early_session(),
        "a plain ring must not answer the offer"
    );
    let tag = ringing.tag().to_owned();
    drain_provisional(&mut responses).await;

    let mut update_responses = caller_endpoint
        .send(
            raw_in_dialog(
                &caller_endpoint,
                &Method::Update,
                CALL_ID,
                &tag,
                2,
                Some(&sdp(40002, 8, "PCMA")),
            ),
            Target::udp(callee_addr),
        )
        .await
        .expect("sends the UPDATE");

    let arrived = tokio::time::timeout(Duration::from_secs(2), callee_incoming.recv())
        .await
        .expect("the UPDATE arrives")
        .expect("a request");
    assert!(ringing.on_update(&arrived).await.expect("handled"));

    let refused = tokio::time::timeout(Duration::from_secs(2), update_responses.final_response())
        .await
        .expect("the UPDATE is answered")
        .expect("a response");
    assert_eq!(
        refused.status.code(),
        500,
        "an offer arriving while we owe an answer must be 500, not {}",
        refused.status.code()
    );
    let retry = refused
        .headers
        .value(&HeaderName::RetryAfter)
        .expect("§5.2 requires Retry-After here, or the peer learns only that it failed");
    let seconds: u64 = String::from_utf8_lossy(&retry)
        .trim()
        .parse()
        .expect("a delta-seconds");
    assert!(
        seconds <= 10,
        "§5.2 asks for a value between 0 and 10 seconds, got {seconds}"
    );
}

/// A refresh carries no description, so it collides with nothing — even in the dialog above,
/// where an *offer* would have been refused. Without this the RFC 4028 §7.4 refresh would be
/// refusable for a reason that has nothing to do with whether the far end is alive.
#[tokio::test]
async fn an_update_with_no_offer_is_accepted_even_when_an_answer_is_owed() {
    const CALL_ID: &str = "update-3@sipx";
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, _caller_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    let mut responses = caller_endpoint
        .send(
            raw_invite(&caller_endpoint, CALL_ID, &sdp(40000, 0, "PCMU")),
            Target::udp(callee_addr),
        )
        .await
        .expect("sends");
    let invite = callee_incoming.recv().await.expect("the INVITE arrives");
    let mut ringing = sipx_call::ring(&callee_endpoint, &invite, 180, "Ringing", true)
        .await
        .expect("rings");
    let tag = ringing.tag().to_owned();
    drain_provisional(&mut responses).await;

    let mut update_responses = caller_endpoint
        .send(
            raw_in_dialog(&caller_endpoint, &Method::Update, CALL_ID, &tag, 2, None),
            Target::udp(callee_addr),
        )
        .await
        .expect("sends the UPDATE");
    let arrived = tokio::time::timeout(Duration::from_secs(2), callee_incoming.recv())
        .await
        .expect("the UPDATE arrives")
        .expect("a request");
    assert!(ringing.on_update(&arrived).await.expect("handled"));

    let answered = tokio::time::timeout(Duration::from_secs(2), update_responses.final_response())
        .await
        .expect("the UPDATE is answered")
        .expect("a response");
    assert_eq!(answered.status.code(), 200);
    assert!(
        answered.body().is_empty(),
        "an UPDATE with no offer must not be answered with a description"
    );
}

/// §5.2 and `M-8`'s rule together: a description this side cannot use is refused 488 and the
/// dialog carries on. An early dialog torn down over an unusable renegotiation would be a call
/// lost while it was still ringing.
#[tokio::test]
async fn an_unacceptable_description_is_refused_488_and_the_dialog_survives() {
    const CALL_ID: &str = "update-4@sipx";
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, _caller_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    let mut responses = caller_endpoint
        .send(
            raw_invite(&caller_endpoint, CALL_ID, &sdp(40000, 0, "PCMU")),
            Target::udp(callee_addr),
        )
        .await
        .expect("sends");
    let invite = callee_incoming.recv().await.expect("the INVITE arrives");
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
    drain_provisional(&mut responses).await;

    // G.722, which sipx does not carry. The offer is well formed and unusable, which is exactly
    // the case 488 exists for.
    let mut update_responses = caller_endpoint
        .send(
            raw_in_dialog(
                &caller_endpoint,
                &Method::Update,
                CALL_ID,
                &tag,
                2,
                Some(&sdp(40002, 9, "G722")),
            ),
            Target::udp(callee_addr),
        )
        .await
        .expect("sends the UPDATE");
    let arrived = tokio::time::timeout(Duration::from_secs(2), callee_incoming.recv())
        .await
        .expect("the UPDATE arrives")
        .expect("a request");
    assert!(ringing.on_update(&arrived).await.expect("handled"));

    let refused = tokio::time::timeout(Duration::from_secs(2), update_responses.final_response())
        .await
        .expect("the UPDATE is answered")
        .expect("a response");
    assert_eq!(refused.status.code(), 488);

    // The dialog survived it: a second, usable UPDATE is accepted afterwards rather than
    // refused as a leftover exchange, and the call still answers on what it settled.
    let mut again = caller_endpoint
        .send(
            raw_in_dialog(
                &caller_endpoint,
                &Method::Update,
                CALL_ID,
                &tag,
                3,
                Some(&sdp(40004, 8, "PCMA")),
            ),
            Target::udp(callee_addr),
        )
        .await
        .expect("sends the second UPDATE");
    let arrived = tokio::time::timeout(Duration::from_secs(2), callee_incoming.recv())
        .await
        .expect("the second UPDATE arrives")
        .expect("a request");
    assert!(ringing.on_update(&arrived).await.expect("handled"));
    let accepted = tokio::time::timeout(Duration::from_secs(2), again.final_response())
        .await
        .expect("answered")
        .expect("a response");
    assert_eq!(
        accepted.status.code(),
        200,
        "the 488 left the exchange open, so the next UPDATE was refused too"
    );

    let call = sipx_call::answer_early(&callee_endpoint, &invite, &mut ringing)
        .await
        .expect("answers");
    assert!(!call.is_ended());
    assert_eq!(call.media().codec(), sipx_media::Codec::Pcma);
}

/// Read provisionals off a transaction until one that establishes a dialog has arrived.
async fn drain_provisional(responses: &mut sipx_transport::Responses) -> sipx_sip::Response {
    tokio::time::timeout(Duration::from_secs(2), async {
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
    .expect("a provisional establishes the early dialog")
}

/// An INVITE from a peer that names a session timer, so sipx answers as the refresher.
///
/// RFC 4028 Table 2 row 4: the caller expressed no preference, so the answering side takes the
/// job — which makes sipx the side that has to choose a method for the refresh.
fn timed_invite(endpoint: &Handle, call_id: &'static str, allow: &'static str) -> Request {
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
        .header(HeaderName::Supported, bytes::Bytes::from_static(b"timer"))
        .expect("supported")
        .header(HeaderName::SessionExpires, bytes::Bytes::from_static(b"90"))
        .expect("session-expires")
        .header(
            HeaderName::Allow,
            bytes::Bytes::from_static(allow.as_bytes()),
        )
        .expect("allow")
        .header(HeaderName::Contact, contact(endpoint))
        .expect("contact")
        .header(
            HeaderName::ContentType,
            bytes::Bytes::from_static(b"application/sdp"),
        )
        .expect("content-type")
        .max_forwards(70)
        .body(bytes::Bytes::from(sdp(40000, 0, "PCMU")))
        .build()
}

/// Answer the next in-dialog request with a bare 200, and report what it was.
async fn answer_next(endpoint: &Handle, incoming: &mut Receiver<Incoming>) -> Request {
    let arrived = tokio::time::timeout(Duration::from_secs(5), incoming.recv())
        .await
        .expect("a refresh arrives")
        .expect("a request");
    let response = sipx_sip::build::ResponseBuilder::to_request(
        &arrived.request,
        sipx_sip::StatusCode::new(200).expect("200"),
        "OK",
    )
    .expect("builds")
    .header(HeaderName::Contact, contact(endpoint))
    .expect("contact")
    .build();
    endpoint
        .respond(&arrived.key, response)
        .await
        .expect("responds");
    arrived.request.clone()
}

/// Move the clock past the refresh deadline, then put it back.
///
/// Paused only for the jump. The refresh that follows is a real round trip over a real socket,
/// and a paused clock underneath one is a test that passes or hangs depending on scheduling.
async fn pass_the_refresh_deadline() {
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(46)).await;
    tokio::time::resume();
}

/// RFC 4028 §7.4: "If a UAC knows that its peer supports the UPDATE method, it is RECOMMENDED
/// that UPDATE be used instead of a re-INVITE."
///
/// Knowing means the peer's `Allow` (RFC 3311 §4), and nothing else — so the same call refreshes
/// two different ways depending only on that header.
#[tokio::test]
async fn a_refresh_uses_update_when_the_peer_allows_it() {
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, mut caller_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    let _responses = caller_endpoint
        .send(
            timed_invite(
                &caller_endpoint,
                "update-5@sipx",
                "INVITE, ACK, CANCEL, BYE, UPDATE",
            ),
            Target::udp(callee_addr),
        )
        .await
        .expect("sends");
    let invite = callee_incoming.recv().await.expect("the INVITE arrives");
    let mut call = sipx_call::answer(&callee_endpoint, &invite, loopback())
        .await
        .expect("answers");
    assert_eq!(
        call.session_interval().map(|(_, refresh)| refresh),
        Some(true),
        "sipx must be the refresher for this test to say anything"
    );

    pass_the_refresh_deadline().await;
    let refreshing = tokio::spawn(async move {
        let request = answer_next(&caller_endpoint, &mut caller_incoming).await;
        (request, caller_endpoint, caller_incoming)
    });
    call.on_session_deadline().await.expect("the refresh lands");
    let (refresh, caller_endpoint, mut caller_incoming) =
        refreshing.await.expect("the peer answered");

    assert_eq!(
        refresh.method,
        Method::Update,
        "the peer said it allows UPDATE and got a re-INVITE anyway"
    );
    assert!(
        refresh.body().is_empty(),
        "a refresh must carry no description: re-offering an unchanged session puts a liveness \
         check under the offer/answer rules it has no business being under"
    );
    let expires = refresh
        .headers
        .value(&HeaderName::SessionExpires)
        .expect("§7.4: the refresh names the interval in force");
    assert!(String::from_utf8_lossy(&expires).contains("90"));

    // §5.1 all the same: an explicit renegotiation of a *confirmed* dialog is still a
    // re-INVITE, because an UPDATE must be answered promptly and leaves no window in which a
    // user could be asked. `M-8`'s behaviour does not change because UPDATE became available.
    let renegotiating =
        tokio::spawn(async move { answer_next(&caller_endpoint, &mut caller_incoming).await });
    let _ = call.reinvite(sipx_sdp::Direction::SendRecv).await;
    let renegotiation = renegotiating.await.expect("the peer answered");
    assert_eq!(
        renegotiation.method,
        Method::Invite,
        "a confirmed-dialog renegotiation stopped being a re-INVITE"
    );
}

/// And the other half of §7.4: a peer that never said it allows UPDATE gets `S-11`'s re-INVITE.
///
/// Guessing the other way costs a working call — a refresh answered 405 is a refresh that never
/// happens, and the deadline behind it hangs up on a peer that is alive.
#[tokio::test]
async fn a_refresh_falls_back_to_a_reinvite_when_the_peer_does_not() {
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, mut caller_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    let _responses = caller_endpoint
        .send(
            timed_invite(
                &caller_endpoint,
                "update-6@sipx",
                "INVITE, ACK, CANCEL, BYE",
            ),
            Target::udp(callee_addr),
        )
        .await
        .expect("sends");
    let invite = callee_incoming.recv().await.expect("the INVITE arrives");
    let mut call = sipx_call::answer(&callee_endpoint, &invite, loopback())
        .await
        .expect("answers");

    pass_the_refresh_deadline().await;
    let refreshing =
        tokio::spawn(async move { answer_next(&caller_endpoint, &mut caller_incoming).await });
    let _ = call.on_session_deadline().await;
    let refresh = refreshing.await.expect("the peer answered");

    assert_eq!(
        refresh.method,
        Method::Invite,
        "a peer that never advertised UPDATE was sent one"
    );
}

/// RFC 3311 §4, on the two messages the section names: the INVITE and its 2xx.
///
/// The peer's `Allow` is the only permission there is, so a message that omits UPDATE is a
/// standing instruction to the far end never to send one — invisible from this side, which is
/// why it is asserted on the wire rather than inferred from a constant.
#[tokio::test]
async fn allow_lists_update_on_the_invite_and_on_its_responses() {
    let (peer_endpoint, mut peer_incoming) = endpoint().await;
    let (caller_endpoint, _caller_incoming) = endpoint().await;
    let peer_addr = peer_endpoint.local_addr();

    let seen = tokio::spawn(async move { peer_incoming.recv().await.expect("an INVITE") });
    let _ = sipx_call::dial(
        &caller_endpoint,
        Target::udp(peer_addr),
        &to_uri(),
        &sipx_call::DialOptions::new("<sip:caller@example.net>", loopback())
            .with_timeout(Duration::from_millis(300)),
    )
    .await;
    let invite = seen.await.expect("the INVITE arrives").request;
    assert!(
        sipx_sip::update::peer_allows(&invite.headers),
        "the INVITE did not list UPDATE, so no compliant peer will ever send one: {:?}",
        invite
            .headers
            .value(&HeaderName::Allow)
            .map(|v| String::from_utf8_lossy(&v).into_owned())
    );

    // And the 2xx, which is where a UAC learns it.
    const CALL_ID: &str = "update-7@sipx";
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (peer_endpoint, _peer_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();
    let mut responses = peer_endpoint
        .send(
            raw_invite(&peer_endpoint, CALL_ID, &sdp(40000, 0, "PCMU")),
            Target::udp(callee_addr),
        )
        .await
        .expect("sends");
    let arrived = callee_incoming.recv().await.expect("the INVITE arrives");
    let _call = sipx_call::answer(&callee_endpoint, &arrived, loopback())
        .await
        .expect("answers");
    let ok = tokio::time::timeout(Duration::from_secs(2), responses.final_response())
        .await
        .expect("a final response")
        .expect("a response");
    assert_eq!(ok.status.code(), 200);
    assert!(
        sipx_sip::update::peer_allows(&ok.headers),
        "the 2xx did not list UPDATE"
    );
}

/// The other direction of §5.1: sipx *sends* an UPDATE, in an early dialog and in a confirmed
/// one. Receiving one is half of "MAY be sent for both early and confirmed dialogs"; a stack
/// that can only receive them cannot renegotiate anything itself.
#[tokio::test]
async fn sipx_sends_an_update_in_an_early_dialog_and_in_a_confirmed_one() {
    const CALL_ID: &str = "update-8@sipx";
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, mut caller_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    let mut responses = caller_endpoint
        .send(
            raw_invite(&caller_endpoint, CALL_ID, &sdp(40000, 0, "PCMU")),
            Target::udp(callee_addr),
        )
        .await
        .expect("sends");
    let invite = callee_incoming.recv().await.expect("the INVITE arrives");
    let mut ringing = sipx_call::ring_early(
        &callee_endpoint,
        &invite,
        183,
        "Session Progress",
        loopback(),
    )
    .await
    .expect("rings with an answer");
    drain_provisional(&mut responses).await;

    // Early: the invitation has no final response, and the session changes anyway.
    let peer = tokio::spawn(async move {
        let request = answer_next(&caller_endpoint, &mut caller_incoming).await;
        (request, caller_endpoint, caller_incoming)
    });
    ringing
        .update(sipx_sdp::Direction::SendOnly)
        .await
        .expect("the early UPDATE is accepted");
    let (early, caller_endpoint, mut caller_incoming) = peer.await.expect("the peer answered");

    assert_eq!(early.method, Method::Update);
    assert!(
        String::from_utf8_lossy(early.body()).contains("a=sendonly"),
        "the early UPDATE carried no offer, so it renegotiated nothing: {:?}",
        String::from_utf8_lossy(early.body())
    );
    assert!(
        sipx_sip::update::peer_allows(&early.headers),
        "an UPDATE we send must itself say that we accept them"
    );

    // Confirmed: the same request, once the call is up.
    let mut call = sipx_call::answer_early(&callee_endpoint, &invite, &mut ringing)
        .await
        .expect("answers");
    let peer =
        tokio::spawn(async move { answer_next(&caller_endpoint, &mut caller_incoming).await });
    call.update(sipx_sdp::Direction::SendRecv)
        .await
        .expect("the confirmed UPDATE is accepted");
    let confirmed = peer.await.expect("the peer answered");
    assert_eq!(confirmed.method, Method::Update);
    assert!(!call.is_ended());
}
