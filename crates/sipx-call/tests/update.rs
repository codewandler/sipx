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

/// A [`Peer`] for this pair of endpoints.
fn peer(endpoint: &Handle, callee: std::net::SocketAddr) -> Peer {
    Peer {
        endpoint: endpoint.clone(),
        callee,
    }
}

/// The raw peer: the endpoint it speaks from, and the sipx endpoint it speaks to.
///
/// Bundled because every helper below needs both, and two loose arguments that are each "an
/// address" are two arguments a call site can transpose.
struct Peer {
    endpoint: Handle,
    callee: std::net::SocketAddr,
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
    let early_media_address = ringing
        .media()
        .expect("ring_early starts media")
        .local_addr();

    let provisional = drain_provisional(&mut responses).await;
    assert_early_answer(&provisional);

    // RFC 3262 §5: the answer went out in a reliable provisional, so it has to be acknowledged
    // before the invitation can be accepted. `answer_early` refuses until it is.
    acknowledge(
        &peer(&caller_endpoint, callee_addr),
        &mut callee_incoming,
        &mut ringing,
        CALL_ID,
        &provisional,
        2,
    )
    .await;

    // The renegotiation, inside the early dialog.
    let update = raw_in_dialog(
        &caller_endpoint,
        &Method::Update,
        CALL_ID,
        &tag,
        3,
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
    assert_eq!(
        ringing
            .media()
            .expect("the updated early session is still running")
            .local_addr(),
        early_media_address,
        "applying the UPDATE rebound the address its answer had already advertised"
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
    assert_eq!(call.media().local_addr(), early_media_address);
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
    let provisional = drain_provisional(&mut responses).await;
    acknowledge(
        &peer(&caller_endpoint, callee_addr),
        &mut callee_incoming,
        &mut ringing,
        CALL_ID,
        &provisional,
        2,
    )
    .await;

    // G.722, which sipx does not carry. The offer is well formed and unusable, which is exactly
    // the case 488 exists for.
    let mut update_responses = caller_endpoint
        .send(
            raw_in_dialog(
                &caller_endpoint,
                &Method::Update,
                CALL_ID,
                &tag,
                3,
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
                4,
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

/// What a provisional from `ring_early` must carry: a dialog to address, the answer itself, and
/// RFC 3311 §4's `Allow`, which is where the caller is told it may renegotiate at all.
fn assert_early_answer(provisional: &sipx_sip::Response) {
    assert!(provisional.headers.value(&HeaderName::Contact).is_some());
    let answer = String::from_utf8_lossy(provisional.body()).into_owned();
    assert!(
        answer.contains("m=audio"),
        "the provisional carried no answer, so the session was never negotiated: {answer:?}"
    );
    let allow = provisional
        .headers
        .value(&HeaderName::Allow)
        .expect("a reliable provisional carrying SDP must say what it allows");
    assert!(
        String::from_utf8_lossy(&allow).contains("UPDATE"),
        "the provisional did not advertise UPDATE, so no compliant peer would send one"
    );
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
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (peer_endpoint, _peer_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();
    let mut responses = peer_endpoint
        .send(
            raw_invite(&peer_endpoint, "update-7@sipx", &sdp(40000, 0, "PCMU")),
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
///
/// **The answering side only, and the name now says so.** The "caller" below is a raw peer
/// sending a hand-built INVITE; both send-side assertions go through `ring_early` and
/// `answer_early`, which are UAS handles. The UAC half of the same sentence is
/// `a_caller_renegotiates_before_the_callee_answers`, and until it existed this name promised a
/// path nothing exercised.
#[tokio::test]
async fn sipx_as_uas_sends_an_update_in_an_early_dialog_and_in_a_confirmed_one() {
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
    let provisional = drain_provisional(&mut responses).await;
    acknowledge(
        &peer(&caller_endpoint, callee_addr),
        &mut callee_incoming,
        &mut ringing,
        CALL_ID,
        &provisional,
        2,
    )
    .await;

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

/// The reliable `183` a callee sends when it means to answer the offer before the call is.
///
/// Every header on it is load-bearing for RFC 3311 §5.1. The `To` tag creates the early dialog
/// (RFC 3261 §12.1.1); `Require: 100rel` with an `RSeq` makes it the one response RFC 3262 §5
/// allows an answer to travel in; the `Contact` is where the caller's in-dialog requests go; and
/// the `Allow` is §4's permission to send an UPDATE at all.
///
/// `body` is optional because the *absence* of a description is a case in its own right (`S-25`):
/// a reliable provisional that describes nothing establishes the dialog, answers no offer, and is
/// not a failure.
fn early_answer(
    peer: &Handle,
    invite: &Incoming,
    tag: &str,
    body: Option<&str>,
) -> sipx_sip::Response {
    let to = String::from_utf8_lossy(
        &invite
            .request
            .headers
            .value(&HeaderName::To)
            .expect("the INVITE carries a To"),
    )
    .into_owned();
    let builder = sipx_sip::build::ResponseBuilder::to_request(
        &invite.request,
        sipx_sip::StatusCode::new(183).expect("a valid status"),
        "Session Progress",
    )
    .expect("builds")
    .set_header(
        &HeaderName::To,
        bytes::Bytes::from(format!("{to};tag={tag}")),
    )
    .expect("to")
    .header(HeaderName::Contact, contact(peer))
    .expect("contact")
    .header(
        HeaderName::Allow,
        bytes::Bytes::from_static(b"INVITE, ACK, CANCEL, BYE, PRACK, UPDATE"),
    )
    .expect("allow")
    .header(HeaderName::Require, bytes::Bytes::from_static(b"100rel"))
    .expect("require")
    .header(HeaderName::RSeq, bytes::Bytes::from_static(b"1"))
    .expect("rseq");
    let Some(body) = body else {
        return builder.build();
    };
    builder
        .header(
            HeaderName::ContentType,
            bytes::Bytes::from_static(b"application/sdp"),
        )
        .expect("content-type")
        .body(bytes::Bytes::from(body.to_owned()))
        .build()
}

/// A `200` to an in-dialog request the peer received, carrying a description when it needs one.
///
/// The `To` is copied from the request, which already carries this side's tag — sipx built the
/// request from the dialog the `183` created, so a fresh tag here would answer in a dialog that
/// does not exist.
fn ok_to(incoming: &Incoming, body: Option<&str>) -> sipx_sip::Response {
    let mut builder = sipx_sip::build::ResponseBuilder::to_request(
        &incoming.request,
        sipx_sip::StatusCode::new(200).expect("a valid status"),
        "OK",
    )
    .expect("builds")
    .header(
        HeaderName::Allow,
        bytes::Bytes::from_static(b"INVITE, ACK, CANCEL, BYE, PRACK, UPDATE"),
    )
    .expect("allow");
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

/// An in-dialog request from the **callee** of a dialog sipx placed.
///
/// The mirror of [`raw_in_dialog`], and not a reuse of it: the tags swap with the role. A
/// request from the callee carries the callee in `From` and the caller in `To`, so both are
/// taken from the INVITE sipx sent rather than written out here — the caller's own tag is one
/// sipx chose and this side never sees except on the wire.
fn from_callee(
    peer: &Handle,
    invite: &Request,
    tag: &str,
    method: &Method,
    cseq: u32,
    body: Option<&str>,
) -> Request {
    let copied = |name: &HeaderName| {
        bytes::Bytes::from(
            invite
                .headers
                .value(name)
                .expect("the INVITE carries it")
                .into_owned(),
        )
    };
    let from = {
        let to = String::from_utf8_lossy(
            &invite
                .headers
                .value(&HeaderName::To)
                .expect("the INVITE carries a To"),
        )
        .into_owned();
        bytes::Bytes::from(format!("{to};tag={tag}"))
    };
    let mut builder = sipx_sip::build::RequestBuilder::new(method.clone(), to_uri())
        .header(HeaderName::Via, via(peer))
        .expect("via")
        .header(HeaderName::To, copied(&HeaderName::From))
        .expect("to")
        .header(HeaderName::From, from)
        .expect("from")
        .header(HeaderName::CallId, copied(&HeaderName::CallId))
        .expect("call-id")
        .cseq(cseq, method)
        .expect("cseq")
        .header(HeaderName::Contact, contact(peer))
        .expect("contact")
        .header(
            HeaderName::Allow,
            bytes::Bytes::from_static(b"INVITE, ACK, CANCEL, BYE, PRACK, UPDATE"),
        )
        .expect("allow")
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

/// The story's failing-first test: sipx is the **caller**, and it renegotiates while it rings.
///
/// Every other early-dialog test in this file has sipx answering. RFC 3311 §5.1 says an UPDATE
/// "MAY be sent by either caller or callee", and this is the half that had no way to happen:
/// `dial` awaits the final response inside itself, so there was no moment at which the
/// application held the early dialog, and therefore nothing to send an UPDATE *from*.
///
/// Both directions are here, because both are the Acceptance. The caller sends one and the
/// callee sends one back, and the codec the exchange settles is what the eventual call runs on —
/// asserting that rather than merely that a `200` came back is the difference between this test
/// and one that would also pass if the answer were built and thrown away.
/// Dial a raw peer that rings reliably with the answer, and hand back the early dialog.
///
/// Its own function because it is the *precondition*, not the subject: RFC 3311 §5.1 will not
/// let either end renegotiate until the INVITE's offer/answer exchange has closed, and RFC 3262
/// §5 leaves exactly one way to close it before the 200. Everything here is that one way, and
/// the assertions in it are the ones that would otherwise make a later failure unreadable.
async fn dialing_against(
    peer_endpoint: &Handle,
    peer_incoming: &mut Receiver<Incoming>,
    caller_endpoint: &Handle,
) -> (sipx_call::Dialing, Incoming) {
    let peer_addr = peer_endpoint.local_addr();
    // Spawned, because this is the whole point: it comes back while the invitation is still
    // open, and the test would deadlock against a `dial` that waited for the final response.
    let dialling = tokio::spawn({
        let endpoint = caller_endpoint.clone();
        async move {
            sipx_call::dial_early(
                &endpoint,
                Target::udp(peer_addr),
                &to_uri(),
                &sipx_call::DialOptions::new("<sip:caller@example.net>", loopback())
                    .with_timeout(Duration::from_secs(10)),
            )
            .await
        }
    });

    let invite = peer_incoming.recv().await.expect("the INVITE arrives");
    assert!(
        sipx_sip::update::peer_allows(&invite.request.headers),
        "RFC 3311 §4: a UAC 'SHOULD also include an Allow header field in the INVITE request, \
         listing the method UPDATE' — without it the callee may never send us one"
    );

    // The answer travels in a reliable provisional, which is the only place RFC 3262 §5 allows
    // one before the 200 — and without it RFC 3311 §5.1 would forbid the caller's own offer
    // outright, because its INVITE offer would still be unanswered.
    peer_endpoint
        .respond(
            &invite.key,
            early_answer(
                peer_endpoint,
                &invite,
                "callee-tag",
                Some(&sdp(40000, 0, "PCMU")),
            ),
        )
        .await
        .expect("rings with an answer");

    let prack = peer_incoming.recv().await.expect("the PRACK arrives");
    assert_eq!(
        prack.request.method,
        Method::Prack,
        "the caller did not acknowledge the reliable provisional (RFC 3262 §4)"
    );
    assert!(
        prack.request.body().is_empty(),
        "RFC 3262 §5: the INVITE carried the offer, so the provisional carried the answer and \
         the PRACK owes no description — a second one would start an exchange nobody asked for"
    );
    peer_endpoint
        .respond(&prack.key, ok_to(&prack, None))
        .await
        .expect("answers the PRACK");

    let dialing = tokio::time::timeout(Duration::from_secs(5), dialling)
        .await
        .expect("dial_early returns while the invitation is still open")
        .expect("joins")
        .expect("an early dialog");
    assert!(
        dialing.has_early_session(),
        "the reliable provisional carried the answer, so the early session is renegotiable \
         (RFC 3311 §5.1, RFC 3262 §5)"
    );
    assert!(
        dialing.peer_allows_update(),
        "the provisional's Allow listed UPDATE, so the caller may renegotiate (RFC 3311 §4)"
    );
    (dialing, invite)
}

/// The other direction: the *callee* renegotiates the caller's early dialog, and the caller
/// answers it under §5.2 — the same three refusal rules, because it is the same code.
///
/// The 2xx's body is what is asserted, not merely its status. A handle that recognised the
/// UPDATE, answered `200` and described nothing would have renegotiated nothing.
async fn the_callee_renegotiates(
    peer_endpoint: &Handle,
    caller_endpoint: &Handle,
    caller_incoming: &mut Receiver<Incoming>,
    invite: &Incoming,
    dialing: &mut sipx_call::Dialing,
) {
    let mut received = peer_endpoint
        .send(
            from_callee(
                peer_endpoint,
                &invite.request,
                "callee-tag",
                &Method::Update,
                1,
                Some(&sdp(40004, 8, "PCMA")),
            ),
            Target::udp(caller_endpoint.local_addr()),
        )
        .await
        .expect("sends an UPDATE into the early dialog");
    let arrived = tokio::time::timeout(Duration::from_secs(2), caller_incoming.recv())
        .await
        .expect("the UPDATE arrives")
        .expect("a request");
    assert!(
        dialing.on_update(&arrived).await.expect("handled"),
        "the UPDATE was not recognised as belonging to the caller's early dialog"
    );
    let answered = tokio::time::timeout(Duration::from_secs(2), received.final_response())
        .await
        .expect("the UPDATE is answered")
        .expect("a response");
    assert_eq!(
        answered.status.code(),
        200,
        "a renegotiation of the caller's early dialog was refused: {} {}",
        answered.status.code(),
        String::from_utf8_lossy(&answered.reason)
    );
    let body = String::from_utf8_lossy(answered.body()).into_owned();
    assert!(
        body.contains("RTP/AVP 8"),
        "the caller's 2xx carried no answer to the callee's offer: {body:?}"
    );
}

/// The `200` accepting the invitation.
///
/// `None` is the usual case here and is deliberate: the answer went in the reliable provisional
/// and the UPDATEs settled everything since, so a description would be either a second answer or
/// an undone renegotiation. `answer_early` omits it in this exact case, from the other side. A
/// body is only for the far end that rang *without* describing anything, which leaves the INVITE's
/// offer for the 2xx to answer.
fn accepted(peer_endpoint: &Handle, invite: &Incoming, body: Option<&str>) -> sipx_sip::Response {
    let builder = sipx_sip::build::ResponseBuilder::to_request(
        &invite.request,
        sipx_sip::StatusCode::new(200).expect("a valid status"),
        "OK",
    )
    .expect("builds")
    .set_header(
        &HeaderName::To,
        bytes::Bytes::from("<sip:callee.example>;tag=callee-tag"),
    )
    .expect("to")
    .header(HeaderName::Contact, contact(peer_endpoint))
    .expect("contact")
    .header(
        HeaderName::Allow,
        bytes::Bytes::from_static(b"INVITE, ACK, CANCEL, BYE, PRACK, UPDATE"),
    )
    .expect("allow");
    let Some(body) = body else {
        return builder.build();
    };
    builder
        .header(
            HeaderName::ContentType,
            bytes::Bytes::from_static(b"application/sdp"),
        )
        .expect("content-type")
        .body(bytes::Bytes::from(body.to_owned()))
        .build()
}

#[tokio::test]
async fn a_caller_renegotiates_before_the_callee_answers() {
    let (peer_endpoint, mut peer_incoming) = endpoint().await;
    let (caller_endpoint, mut caller_incoming) = endpoint().await;

    let (mut dialing, invite) =
        dialing_against(&peer_endpoint, &mut peer_incoming, &caller_endpoint).await;

    // Send: an offer out of the caller's own early dialog. Answered PCMA, so the renegotiation
    // is visible later as a codec the INVITE never settled on.
    let answering = tokio::spawn(async move {
        let update = peer_incoming.recv().await.expect("the UPDATE arrives");
        peer_endpoint
            .respond(&update.key, ok_to(&update, Some(&sdp(40002, 8, "PCMA"))))
            .await
            .expect("answers the UPDATE");
        (update, peer_endpoint, peer_incoming)
    });
    dialing
        .update(sipx_sdp::Direction::SendOnly)
        .await
        .expect("the caller's early UPDATE is accepted");
    // Bounded, because the failure this guards against is a `update` that sends nothing: the
    // peer then waits forever and the test hangs rather than failing, which is the one outcome
    // a suite cannot report.
    let (sent, peer_endpoint, mut peer_incoming) =
        tokio::time::timeout(Duration::from_secs(2), answering)
            .await
            .expect("the caller's UPDATE reached the peer")
            .expect("the peer answered");

    assert_eq!(sent.request.method, Method::Update);
    assert!(
        String::from_utf8_lossy(sent.request.body()).contains("a=sendonly"),
        "the caller's UPDATE carried no offer, so it renegotiated nothing: {:?}",
        String::from_utf8_lossy(sent.request.body())
    );
    assert!(
        sipx_sip::update::peer_allows(&sent.request.headers),
        "an UPDATE we send must itself say that we accept them (RFC 3311 §4)"
    );

    // Receive: the callee renegotiates the same early dialog, and the caller applies §5.2's
    // rules to it — the same three the answering side already applies.
    the_callee_renegotiates(
        &peer_endpoint,
        &caller_endpoint,
        &mut caller_incoming,
        &invite,
        &mut dialing,
    )
    .await;

    // And now the call is accepted. Its 200 carries no description at all: the answer went in
    // the provisional and the UPDATEs settled everything since, so a body here would be either
    // a second answer or an undone renegotiation — which is exactly what `answer_early` sends.
    peer_endpoint
        .respond(&invite.key, accepted(&peer_endpoint, &invite, None))
        .await
        .expect("answers the invitation");

    let call = tokio::time::timeout(Duration::from_secs(5), dialing.answered())
        .await
        .expect("the invitation is answered")
        .expect("a call");
    assert_eq!(
        call.media().codec(),
        sipx_media::Codec::Pcma,
        "the call runs on the codec the INVITE offered, not the one the early UPDATEs settled"
    );
    assert!(!call.is_ended());

    // RFC 3261 §13.2.2.4: without the ACK the callee retransmits its 200 for 32 seconds into a
    // call that is already up, and then tears it down.
    let ack = tokio::time::timeout(Duration::from_secs(2), peer_incoming.recv())
        .await
        .expect("the ACK arrives")
        .expect("a request");
    assert_eq!(ack.request.method, Method::Ack);
}

/// PRACK the reliable provisional that carried the answer, and let sipx handle it.
///
/// Not optional bookkeeping in these tests: RFC 3262 §5 forbids the 2xx while a provisional
/// carrying a description is unacknowledged, so `answer_early` refuses until this has happened.
async fn acknowledge(
    peer: &Peer,
    callee_incoming: &mut Receiver<Incoming>,
    ringing: &mut sipx_call::Ringing,
    call_id: &'static str,
    provisional: &sipx_sip::Response,
    cseq: u32,
) {
    let rseq = provisional
        .headers
        .typed::<sipx_sip::rel::RSeq>()
        .expect("a reliable provisional carries RSeq")
        .expect("it parses")
        .0;
    let tag = ringing.tag().to_owned();
    let prack = sipx_sip::build::RequestBuilder::new(Method::Prack, to_uri())
        .header(HeaderName::Via, via(&peer.endpoint))
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
        .cseq(cseq, &Method::Prack)
        .expect("cseq")
        .header(
            HeaderName::RAck,
            bytes::Bytes::from(format!("{rseq} 1 INVITE")),
        )
        .expect("rack")
        .max_forwards(70)
        .build();
    let _ = peer
        .endpoint
        .send(prack, Target::udp(peer.callee))
        .await
        .expect("sends the PRACK");
    let arrived = tokio::time::timeout(Duration::from_secs(2), callee_incoming.recv())
        .await
        .expect("the PRACK arrives")
        .expect("a request");
    assert!(
        ringing.on_prack(&arrived).await.expect("handled"),
        "the PRACK did not acknowledge the provisional"
    );
    assert!(ringing.is_acknowledged());
}

/// A session refresh must not settle a debt it never took on (RFC 3311 §5.2 rule 3).
///
/// The reachable form of the defect: an early dialog owes an answer to the INVITE's offer, an
/// ordinary RFC 4028 §7.4 refresh comes through first, and the offer-carrying UPDATE behind it
/// is then answered **488** — telling the peer its description was unusable when the description
/// was fine and the moment was not. The refresh is the most ordinary thing a peer sends, so this
/// is not an exotic ordering.
#[tokio::test]
async fn a_refresh_does_not_erase_the_debt_the_invite_left() {
    const CALL_ID: &str = "update-9@sipx";
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
    // Rung the plain way, so the INVITE's offer is unanswered and stays that way.
    let mut ringing = sipx_call::ring(&callee_endpoint, &invite, 180, "Ringing", true)
        .await
        .expect("rings");
    let tag = ringing.tag().to_owned();
    drain_provisional(&mut responses).await;

    // The refresh. Accepted, correctly — it carries no description and collides with nothing.
    let refresh = exchange_update(
        &peer(&caller_endpoint, callee_addr),
        &mut callee_incoming,
        &mut ringing,
        CALL_ID,
        &tag,
        2,
        None,
    )
    .await;
    assert_eq!(refresh.status.code(), 200);

    // And now the offer. The debt from the INVITE is still outstanding, so §5.2 rule 3 applies.
    let offered = exchange_update(
        &peer(&caller_endpoint, callee_addr),
        &mut callee_incoming,
        &mut ringing,
        CALL_ID,
        &tag,
        3,
        Some(&sdp(40002, 8, "PCMA")),
    )
    .await;
    assert_eq!(
        offered.status.code(),
        500,
        "the refresh cancelled the INVITE's outstanding offer, so an offer that should have \
         been told it was early was told its description was unusable instead"
    );
    assert!(
        offered.headers.value(&HeaderName::RetryAfter).is_some(),
        "§5.2 requires Retry-After on this 500"
    );
}

/// RFC 3261 §12.2.2 governs the early dialog too, and the dialog's sequence number only ever
/// moves forward.
///
/// The second assertion is the serious one. An UPDATE that rolled the recorded number backwards
/// would leave a replayed BYE looking in order — ending a call that is still running, which is
/// the failure `Call::handle` refuses on the confirmed path in as many words. A new path that
/// sidesteps an existing guard is worse than no guard.
#[tokio::test]
async fn an_early_update_from_behind_the_sequence_is_refused_and_does_not_roll_it_back() {
    const CALL_ID: &str = "update-10@sipx";
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
    let provisional = drain_provisional(&mut responses).await;
    acknowledge(
        &peer(&caller_endpoint, callee_addr),
        &mut callee_incoming,
        &mut ringing,
        CALL_ID,
        &provisional,
        2,
    )
    .await;

    // Far ahead, which §12.2.2 explicitly allows: "higher than the remote sequence number by
    // more than one".
    let ahead = exchange_update(
        &peer(&caller_endpoint, callee_addr),
        &mut callee_incoming,
        &mut ringing,
        CALL_ID,
        &tag,
        9,
        None,
    )
    .await;
    assert_eq!(ahead.status.code(), 200);

    // Behind it. §12.2.2: rejected with a 500, not applied.
    let behind = exchange_update(
        &peer(&caller_endpoint, callee_addr),
        &mut callee_incoming,
        &mut ringing,
        CALL_ID,
        &tag,
        5,
        None,
    )
    .await;
    assert_eq!(
        behind.status.code(),
        500,
        "an out-of-order UPDATE was applied in the early dialog"
    );
}

/// And the consequence, which is the part that costs a call: the number the `Call` inherits from
/// the early dialog is the highest one reached, so a replayed BYE from behind it is refused.
///
/// Its own test rather than a tail on the one above, because the two assertions fail
/// independently: a missing ordering check shows up there, and a sequence number that *assigns*
/// rather than advances shows up only here.
#[tokio::test]
async fn a_bye_replayed_from_behind_the_early_dialogs_sequence_does_not_end_the_call() {
    const CALL_ID: &str = "update-12@sipx";
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
    let provisional = drain_provisional(&mut responses).await;
    acknowledge(
        &peer(&caller_endpoint, callee_addr),
        &mut callee_incoming,
        &mut ringing,
        CALL_ID,
        &provisional,
        2,
    )
    .await;

    // Nine, then five. What the five is *answered* is the other test's business; what matters
    // here is that it must not become the dialog's idea of where the sequence has got to.
    let _ = exchange_update(
        &peer(&caller_endpoint, callee_addr),
        &mut callee_incoming,
        &mut ringing,
        CALL_ID,
        &tag,
        9,
        None,
    )
    .await;
    let _ = exchange_update(
        &peer(&caller_endpoint, callee_addr),
        &mut callee_incoming,
        &mut ringing,
        CALL_ID,
        &tag,
        5,
        None,
    )
    .await;

    let mut call = sipx_call::answer_early(&callee_endpoint, &invite, &mut ringing)
        .await
        .expect("answers");

    // Six: behind the 9 that was accepted, but ahead of the 5 that was not. A dialog that let
    // its number roll back to 5 reads this as in order and ends the call; one that kept 9
    // refuses it. A BYE below both would be refused either way and would prove nothing.
    let bye = raw_in_dialog(&caller_endpoint, &Method::Bye, CALL_ID, &tag, 6, None);
    let mut bye_responses = caller_endpoint
        .send(bye, Target::udp(callee_addr))
        .await
        .expect("sends the BYE");
    let arrived = tokio::time::timeout(Duration::from_secs(2), callee_incoming.recv())
        .await
        .expect("the BYE arrives")
        .expect("a request");
    assert!(call.handle(&arrived).await.expect("handled"));
    let answered = tokio::time::timeout(Duration::from_secs(2), bye_responses.final_response())
        .await
        .expect("the BYE is answered")
        .expect("a response");

    assert_eq!(
        answered.status.code(),
        500,
        "a BYE replayed from behind the sequence the early dialog reached was accepted"
    );
    assert!(
        !call.is_ended(),
        "a replayed BYE ended a call that was still running"
    );
}

/// RFC 3262 §5 is a MUST: the 2xx waits for the PRACK when the answer went out in a reliable
/// provisional.
///
/// Not a nicety. The 200 from `answer_early` deliberately carries no description, so a 183 that
/// never arrived would leave the caller holding a confirmed dialog and no answer at all — with
/// no later message that would ever supply one.
#[tokio::test]
async fn the_2xx_waits_for_the_prack_of_an_sdp_carrying_provisional() {
    const CALL_ID: &str = "update-11@sipx";
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
    let provisional = drain_provisional(&mut responses).await;
    assert!(!ringing.is_acknowledged());

    // Answering now would put a 2xx on the wire while the description is unacknowledged.
    let refused = sipx_call::answer_early(&callee_endpoint, &invite, &mut ringing)
        .await
        .map(|_| "a call")
        .map_err(|error| error.to_string());
    assert_eq!(
        refused,
        Err("the reliable provisional carrying the answer has not been acknowledged".to_owned()),
        "the 2xx went out over an unacknowledged provisional"
    );
    // And nothing final reached the caller.
    let leaked = tokio::time::timeout(Duration::from_millis(400), responses.final_response()).await;
    assert!(
        leaked.is_err(),
        "a final response was sent anyway: {:?}",
        leaked.map(|r| r.map(|r| r.status.code()))
    );

    // With the PRACK in hand it goes through, and the early session survives the refusal.
    acknowledge(
        &peer(&caller_endpoint, callee_addr),
        &mut callee_incoming,
        &mut ringing,
        CALL_ID,
        &provisional,
        2,
    )
    .await;
    let call = sipx_call::answer_early(&callee_endpoint, &invite, &mut ringing)
        .await
        .expect("answers once the provisional is acknowledged");
    assert!(!call.is_ended());
}

/// Send an UPDATE from the raw peer, let sipx handle it, and hand back the final response.
async fn exchange_update(
    peer: &Peer,
    callee_incoming: &mut Receiver<Incoming>,
    ringing: &mut sipx_call::Ringing,
    call_id: &'static str,
    tag: &str,
    cseq: u32,
    body: Option<&str>,
) -> sipx_sip::Response {
    let mut responses = peer
        .endpoint
        .send(
            raw_in_dialog(&peer.endpoint, &Method::Update, call_id, tag, cseq, body),
            Target::udp(peer.callee),
        )
        .await
        .expect("sends the UPDATE");
    let arrived = tokio::time::timeout(Duration::from_secs(2), callee_incoming.recv())
        .await
        .expect("the UPDATE arrives")
        .expect("a request");
    assert!(ringing.on_update(&arrived).await.expect("handled"));
    tokio::time::timeout(Duration::from_secs(2), responses.final_response())
        .await
        .expect("the UPDATE is answered")
        .expect("a response")
}

/// A *retransmitted* UPDATE never reaches the transaction user, so §5.2's first rule is about
/// genuinely new transactions and not about a lost response.
///
/// Worth pinning because the distinction decides what "a second UPDATE before the first has a
/// final response" can even mean here. RFC 3261 §17.2.2 has the server transaction resend its
/// last response to a retransmission and tell the TU nothing — so a peer that repeats a request
/// because the answer went missing gets the answer again, not a 500 saying it was too early.
#[tokio::test]
async fn a_retransmitted_update_is_answered_by_the_transaction_layer_alone() {
    const CALL_ID: &str = "update-13@sipx";
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

    // One UPDATE, answered.
    let update = raw_in_dialog(&caller_endpoint, &Method::Update, CALL_ID, &tag, 2, None);
    let mut update_responses = caller_endpoint
        .send(update.clone(), Target::udp(callee_addr))
        .await
        .expect("sends the UPDATE");
    let arrived = tokio::time::timeout(Duration::from_secs(2), callee_incoming.recv())
        .await
        .expect("the UPDATE arrives")
        .expect("a request");
    assert!(ringing.on_update(&arrived).await.expect("handled"));
    let first = tokio::time::timeout(Duration::from_secs(2), update_responses.final_response())
        .await
        .expect("answered")
        .expect("a response");
    assert_eq!(first.status.code(), 200);

    // The same request again, put on the wire without a new client transaction — which is what
    // a retransmission is, and what `send` would *not* produce because it makes a fresh branch.
    caller_endpoint
        .send_directly(update, Target::udp(callee_addr))
        .await
        .expect("retransmits");

    let seen_again = tokio::time::timeout(Duration::from_millis(400), callee_incoming.recv()).await;
    assert!(
        seen_again.is_err(),
        "a retransmitted UPDATE reached the transaction user: {:?}",
        seen_again.map(|r| r.map(|i| i.request.method.clone()))
    );
}

// ── `S-25`: what a caller learns when a reliable provisional cannot be used ───────────────────
//
// Here rather than in a file of their own because the machinery is this file's. An early dialog
// reached with `dial_early` and rung reliably by a raw peer is exactly what `dialing_against`
// builds above, and the failure under test is a step inside that same sequence — the one where
// RFC 3262 §5's answer is read out of the provisional.

/// How long these wait for a message a correct implementation sends at once.
///
/// A bound on failure, not a window to measure in (`X-28`): every wait below is satisfied in
/// microseconds when the code is right, and only a *missing* message ever spends it.
const REFUSAL_BOUND: Duration = Duration::from_secs(5);

/// An answer that is well formed in every respect except the one RFC 4568 §5.1.3 checks.
///
/// The suite is real and the key is a published one (`docs/specs/srtp.md` §10.4), so nothing here
/// fails to parse and nothing fails to negotiate. The tag is `9`; `Capabilities::with_srtp` fixes
/// sipx's own at `1`, so this answers an offer nobody made and the two ends have agreed on
/// nothing. A check on key material alone would miss it.
const ANSWER_KEYED_TO_AN_UNOFFERED_TAG: &str = concat!(
    "v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n",
    "m=audio 41234 RTP/SAVP 0\r\na=rtpmap:0 PCMU/8000\r\n",
    "a=crypto:9 AES_CM_128_HMAC_SHA1_80 ",
    "inline:PS1uQCVeeCFCanVmcjkpPywjNWhcYD0mXXtxaVBR\r\n",
    "a=sendrecv\r\n"
);

/// The key material above, which the error must not repeat back to the application.
const PUBLISHED_KEY: &str = "PS1uQCVeeCFCanVmcjkpPywjNWhcYD0mXXtxaVBR";

/// The next request the peer receives, bounded so a message that never comes fails rather than
/// hangs — the one outcome a suite cannot report.
async fn next_request(incoming: &mut Receiver<Incoming>, what: &str) -> Incoming {
    tokio::time::timeout(REFUSAL_BOUND, incoming.recv())
        .await
        .unwrap_or_else(|_| panic!("{what} never arrived"))
        .unwrap_or_else(|| panic!("{what} never arrived: the endpoint closed first"))
}

/// A caller and a raw peer that speak WSS.
///
/// The rest of this file runs over UDP, and the crypto test below cannot: RFC 4568 §7.1 puts the
/// master key in the SDP body, so sipx offers one only where the signalling protects it
/// (`offered_media`). Over UDP there is no `a=crypto` in the INVITE, an answer carrying one is
/// ignored rather than refused, and §5.1.3's check — the subject there — never runs at all.
struct SecurePair {
    peer: Handle,
    peer_incoming: Receiver<Incoming>,
    caller: Handle,
    /// Held only to keep the caller's endpoint alive; nothing is sent to it.
    _caller_incoming: Receiver<Incoming>,
    target: Target,
}

async fn secure_pair() -> SecurePair {
    let ca = sipx_testkit::certs::Ca::new();
    let (cert, key) = ca.issue_for("localhost");
    let identity = sipx_transport::tls::Identity::from_pem(cert.as_bytes(), key.as_bytes())
        .expect("an identity");

    let mut peer_config = Config::new("127.0.0.1:0".parse().expect("valid"));
    peer_config.wss_server = Some((
        sipx_transport::tls::ServerTls::new(identity).expect("a server"),
        0,
    ));
    let (peer, peer_incoming) = bind(peer_config).await.expect("binds");
    let wss_addr = peer.wss_addr().expect("a WSS port");

    let mut anchors = sipx_transport::tls::TrustAnchors::only();
    anchors.add_pem(ca.pem().as_bytes()).expect("a usable CA");
    let mut caller_config = Config::new("127.0.0.1:0".parse().expect("valid"));
    caller_config.tls_client =
        Some(sipx_transport::tls::ClientTls::new(&anchors).expect("a client"));
    let (caller, caller_incoming) = bind(caller_config).await.expect("binds");

    SecurePair {
        peer,
        peer_incoming,
        caller,
        _caller_incoming: caller_incoming,
        target: Target::new(wss_addr, sipx_transport::TransportKind::Wss).verifying("localhost"),
    }
}

/// The `487` that closes an invitation the caller withdrew (RFC 3261 §9.1).
///
/// Sent so the caller's own withdrawal finishes at once rather than sitting out the grace period
/// it allows for a `200` that crossed the CANCEL. It is what a real callee sends here.
fn terminated(invite: &Incoming) -> sipx_sip::Response {
    sipx_sip::build::ResponseBuilder::to_request(
        &invite.request,
        sipx_sip::StatusCode::new(487).expect("a valid status"),
        "Request Terminated",
    )
    .expect("builds")
    .set_header(
        &HeaderName::To,
        bytes::Bytes::from("<sip:callee.example>;tag=callee-tag"),
    )
    .expect("to")
    .build()
}

/// The story's failing-first test: the refusal reaches the caller, and it withdraws by CANCEL.
///
/// Three things are asserted and all three are the Acceptance. **That** the caller learns of the
/// refusal at all, rather than waiting out a 2xx that never comes — before this story
/// `adopt_early_answer` returned `()` and logged at `debug`, so the only outcome was
/// `Error::Cancelled` when the deadline passed, which says nothing about why. **What** it learns:
/// `Error::Sdp` naming the tag that came back, and not repeating the key material. And **how the
/// invitation ends**: a CANCEL, because RFC 3261 §9.1 is what withdraws an invitation that has
/// only been answered provisionally — there is no final response to ACK, so the ACK-then-BYE
/// `dial` performs after a 2xx has nothing here to attach itself to.
///
/// The peer is hand-built rather than a second sipx because sipx's own answerer echoes the
/// accepted tag correctly (`M-26`) and could not produce this provisional.
#[tokio::test]
async fn a_refused_early_answer_ends_the_invitation_with_a_cancel() {
    let mut pair = secure_pair().await;

    // Spawned: the assertions below are the peer's half of a conversation that has to be running.
    let dialling = tokio::spawn({
        let endpoint = pair.caller.clone();
        let target = pair.target.clone();
        async move {
            sipx_call::dial_early(
                &endpoint,
                target,
                &to_uri(),
                &sipx_call::DialOptions::new("<sip:caller@localhost>", loopback())
                    .with_timeout(Duration::from_secs(30)),
            )
            .await
        }
    });

    let invite = next_request(&mut pair.peer_incoming, "the INVITE").await;
    assert_eq!(invite.request.method, Method::Invite);
    assert!(
        String::from_utf8_lossy(invite.request.body()).contains("a=crypto:1 "),
        "sipx offered no key over WSS, so there is nothing for §5.1.3 to check: {:?}",
        String::from_utf8_lossy(invite.request.body())
    );

    pair.peer
        .respond(
            &invite.key,
            early_answer(
                &pair.peer,
                &invite,
                "callee-tag",
                Some(ANSWER_KEYED_TO_AN_UNOFFERED_TAG),
            ),
        )
        .await
        .expect("rings with an answer");

    // The provisional is acknowledged before it is refused, and deliberately: RFC 3262 §4 makes
    // the PRACK a MUST for any reliable provisional a UAC receives, and the far end retransmits
    // until one arrives. Failing a beat sooner would leave the peer resending a response into a
    // CANCEL that had already gone.
    let prack = next_request(&mut pair.peer_incoming, "the PRACK").await;
    assert_eq!(
        prack.request.method,
        Method::Prack,
        "a refused description must still be acknowledged (RFC 3262 §4)"
    );
    pair.peer
        .respond(&prack.key, ok_to(&prack, None))
        .await
        .expect("answers the PRACK");

    // The failure mode, on the wire. Asserting the *first* request after the PRACK also rules
    // out the shape this story says it is not: an ACK, or a BYE in a dialog no 2xx confirmed.
    let withdrawn = next_request(&mut pair.peer_incoming, "the withdrawal of the invitation").await;
    assert_eq!(
        withdrawn.request.method,
        Method::Cancel,
        "an invitation refused from a provisional is withdrawn with CANCEL (RFC 3261 §9.1), not \
         with {:?}",
        withdrawn.request.method
    );
    pair.peer
        .respond(&withdrawn.key, ok_to(&withdrawn, None))
        .await
        .expect("answers the CANCEL");
    pair.peer
        .respond(&invite.key, terminated(&invite))
        .await
        .expect("terminates the invitation");

    let outcome = tokio::time::timeout(REFUSAL_BOUND, dialling)
        .await
        .expect("the caller stopped waiting for a 2xx that is never coming")
        .expect("joins");
    let error = outcome.err().unwrap_or_else(|| {
        panic!("an early dialog was handed back on an answer keyed to a tag nobody offered")
    });
    let sipx_call::Error::Sdp(reason) = &error else {
        panic!("the refusal reached the application as {error:?}, not as Error::Sdp");
    };
    assert!(
        reason.contains('9'),
        "the error does not say which tag came back: {reason}"
    );
    assert!(
        !reason.contains(PUBLISHED_KEY),
        "the error carries key material: {reason}"
    );
}

/// The other half of the same change: nothing is wrong, and nothing is reported.
///
/// A reliable provisional that describes nothing is ordinary — it establishes the early dialog
/// and answers no offer — so it must leave the invitation running. This is the case an error
/// channel is easiest to break: `adopt_early_answer` returns early for it, and returning early
/// with an error rather than with success would end every call that rings before it answers.
///
/// The 2xx at the end is the assertion that matters. That `dial_early` returned `Ok` only says
/// the refusal did not fire on the way *in*; that the invitation can still be answered is what
/// says it was not withdrawn behind the application's back.
#[tokio::test]
async fn a_reliable_provisional_with_no_description_leaves_the_invitation_running() {
    let (peer_endpoint, mut peer_incoming) = endpoint().await;
    let (caller_endpoint, _caller_incoming) = endpoint().await;
    let peer_addr = peer_endpoint.local_addr();

    let dialling = tokio::spawn({
        let endpoint = caller_endpoint.clone();
        async move {
            sipx_call::dial_early(
                &endpoint,
                Target::udp(peer_addr),
                &to_uri(),
                &sipx_call::DialOptions::new("<sip:caller@example.net>", loopback())
                    .with_timeout(Duration::from_secs(30)),
            )
            .await
        }
    });

    let invite = next_request(&mut peer_incoming, "the INVITE").await;
    peer_endpoint
        .respond(
            &invite.key,
            early_answer(&peer_endpoint, &invite, "callee-tag", None),
        )
        .await
        .expect("rings, describing nothing");

    let prack = next_request(&mut peer_incoming, "the PRACK").await;
    assert_eq!(prack.request.method, Method::Prack);
    peer_endpoint
        .respond(&prack.key, ok_to(&prack, None))
        .await
        .expect("answers the PRACK");

    let dialing = tokio::time::timeout(REFUSAL_BOUND, dialling)
        .await
        .expect("dial_early returns while the invitation is still open")
        .expect("joins")
        .expect("a provisional that describes nothing is not a refusal");
    assert!(
        !dialing.has_early_session(),
        "nothing answered the INVITE's offer, so the exchange is still open (RFC 3311 §5.1)"
    );

    // Still running: the offer the INVITE carried is answered by the 2xx instead, which is the
    // path `Dialing::accept` takes when no provisional settled one.
    let answering = tokio::spawn(dialing.answered());
    peer_endpoint
        .respond(
            &invite.key,
            accepted(&peer_endpoint, &invite, Some(&sdp(40000, 0, "PCMU"))),
        )
        .await
        .expect("answers the invitation");
    let call = tokio::time::timeout(REFUSAL_BOUND, answering)
        .await
        .expect("the invitation was still open when the 2xx arrived")
        .expect("joins")
        .expect("a call");
    assert!(!call.is_ended());
}
