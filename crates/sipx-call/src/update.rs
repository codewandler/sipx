//! UPDATE in a live dialog (RFC 3311).
//!
//! The decision — may this UPDATE be sent, may that one be accepted, and if not which of §5.2's
//! three refusals applies — is in [`sipx_sip::update`], which has no clock and no entropy
//! source. This is the half that puts the result on the wire: the request built inside a
//! dialog, the refusal built with the `Retry-After` §5.2 asks to be *randomly* chosen, and the
//! renegotiation the accepted ones perform.
//!
//! Both an early dialog ([`crate::Ringing`]) and a confirmed one ([`crate::Call`]) use these,
//! which is the point of them being here rather than in either.

use bytes::Bytes;
use sipx_sdp::{Direction, SessionDescription};
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::update::{Reception, Refusal};
use sipx_sip::{HeaderName, Method, Request, StatusCode};
use sipx_transport::{Handle, Incoming, Target};

use crate::call::{Early, add_routes, contact_for};
use crate::dialog::Dialog;
use crate::error::{Error, Result};

/// Whether a request carries a session description, and therefore an offer.
///
/// The body alone is not enough: RFC 3311 §5.1 allows an UPDATE with no description at all —
/// which is exactly what a session refresh is — and a body of another type is not an offer
/// either. Treating a refresh as an offer would put it under §5.2's collision rules and let a
/// liveness check be refused for a reason that has nothing to do with liveness.
#[must_use]
pub(crate) fn carries_offer(request: &Request) -> bool {
    if request.body().is_empty() {
        return false;
    }
    request
        .headers
        .value(&HeaderName::ContentType)
        .is_none_or(|value| {
            // Absent `Content-Type` with a non-empty body is malformed; RFC 3261 §20.15 makes
            // the header mandatory then. Reading it as SDP is the generous choice and the safe
            // one — the description is parsed next, and a body that is not SDP fails there.
            let value = String::from_utf8_lossy(&value).to_ascii_lowercase();
            value.contains("application/sdp")
        })
}

/// Answer an UPDATE that cannot be processed now (RFC 3311 §5.2).
///
/// The dialog is untouched. Every one of these refusals is about *when* the request arrived, so
/// the session it wanted to change carries on exactly as it was.
pub(crate) async fn refuse(endpoint: &Handle, incoming: &Incoming, refusal: Refusal) -> Result<()> {
    let status = StatusCode::new(refusal.status())
        .ok_or_else(|| Error::Sdp("unreachable: literal status".to_owned()))?;
    let mut builder = ResponseBuilder::to_request(&incoming.request, status, refusal.reason())?;
    if refusal.retry_after() {
        // §5.2: "a randomly chosen value between 0 and 10 seconds". Random rather than fixed
        // because both sides may be refusing each other at once, and a constant would have
        // them retry in step forever.
        builder = builder.header(
            HeaderName::RetryAfter,
            Bytes::from(retry_after_seconds().to_string()),
        )?;
    }
    endpoint.respond(&incoming.key, builder.build()).await?;
    Ok(())
}

/// The `Warning` a 488 should carry (RFC 3311 §5.2, RFC 3261 §20.43).
///
/// §5.2 makes this a SHOULD, and it is the difference between a peer that can log why its
/// renegotiation was refused and one that can only log that it was. Warn-code 304 — "Media type
/// not available" — is the one that fits every way sipx reaches a 488 here: a description it
/// could not read, or one whose media it cannot carry.
///
/// The warn-agent is this endpoint's own sent-by, because §20.43's grammar wants a host or a
/// pseudonym and the host is the honest one.
pub(crate) fn warning(endpoint: &Handle) -> String {
    const MEDIA_TYPE_NOT_AVAILABLE: u16 = 304;
    format!(
        "{MEDIA_TYPE_NOT_AVAILABLE} {} \"Media type not available\"",
        endpoint.sent_by_for(sipx_transport::TransportKind::Udp)
    )
}

/// The `Retry-After` for a §5.2 refusal.
///
/// Drawn here rather than in `sipx-sip`, which reads no entropy source: a sans-IO core that
/// reaches for one has stopped being one.
fn retry_after_seconds() -> u64 {
    use rand::Rng as _;
    rand::rng().random_range(0..=sipx_sip::update::RETRY_AFTER_MAX_SECS)
}

/// The start of an UPDATE inside a dialog, and the route set it must travel.
///
/// Returned half-built because what goes on an UPDATE differs by why it was sent: a
/// renegotiation carries a description, a session refresh carries RFC 4028's headers and no
/// body at all. The caller adds those and finishes with [`add_routes`], which must be last so
/// the `Route` headers land in the order RFC 3261 §12.2.1.1 requires.
pub(crate) fn request(
    endpoint: &Handle,
    dialog: &mut Dialog,
    target: &Target,
    body: Option<&SessionDescription>,
) -> Result<(RequestBuilder, Vec<String>)> {
    let (local, remote) = dialog.local_and_remote();
    let cseq = dialog.next_cseq();
    let (uri, routes) = dialog.request_target();

    let mut builder = RequestBuilder::new(Method::Update, uri)
        .header(HeaderName::To, Bytes::from(remote))?
        .header(HeaderName::From, Bytes::from(local))?
        .header(HeaderName::CallId, Bytes::from(dialog.id.call_id.clone()))?
        .cseq(cseq, &Method::Update)?
        // §5.1: UPDATE is a target refresh request, so it carries a `Contact` — and it is how
        // a peer learns this side has moved while the invitation was still ringing.
        .header(
            HeaderName::Contact,
            Bytes::from(contact_for(endpoint, target.transport)),
        )?
        // §4, the other direction: a peer may only send us an UPDATE if we said it could, and
        // an in-dialog request is as good a place to say it as the INVITE was.
        .header(
            HeaderName::Allow,
            Bytes::from_static(sipx_sip::update::ALLOW.as_bytes()),
        )?
        .max_forwards(70);

    if let Some(offer) = body {
        builder = builder
            .header(
                HeaderName::ContentType,
                Bytes::from_static(b"application/sdp"),
            )?
            .body(Bytes::from(offer.to_string_sdp()));
    }
    Ok((builder, routes))
}

/// Send an UPDATE and wait for its final response, whatever that turns out to be.
///
/// The response is handed back rather than turned into an error on a non-2xx, because the two
/// callers need different things out of one: a 422 carries the `Min-SE` a session refresh has
/// to retry at, and a 491 tells a renegotiation to back off rather than to give up.
///
/// There is no ACK. UPDATE is a non-INVITE transaction, so the far end's transaction layer
/// retransmits its final response and this side simply reads it — the retransmit-the-2xx-until-
/// ACK dance a re-INVITE needs does not apply here.
pub(crate) async fn send(
    endpoint: &Handle,
    request: Request,
    target: Target,
) -> Result<sipx_sip::Response> {
    let mut responses = endpoint.send(request, target).await?;
    responses.final_response().await.ok_or(Error::NoResponse)
}

/// The rejection a non-2xx final response amounts to.
pub(crate) fn rejected(response: &sipx_sip::Response) -> Error {
    Error::Rejected {
        status: response.status.code(),
        reason: String::from_utf8_lossy(&response.reason).into_owned(),
    }
}

/// Finish an UPDATE request that needs no headers beyond the ones [`request`] wrote.
pub(crate) fn finish(builder: RequestBuilder, routes: &[String]) -> Result<Request> {
    Ok(add_routes(builder, routes)?.build())
}

/// One early dialog's mutable parts, borrowed for the length of one UPDATE.
///
/// The two roles keep these fields in different structs — [`crate::Ringing`] for the side that
/// was called, [`crate::Dialing`](crate::Dialing) for the side that called — and RFC 3311 draws
/// no distinction between them: §5.1 says an UPDATE "MAY be sent for both early and confirmed
/// dialogs, and MAY be sent by either caller or callee", and §5.2's three refusals are the same
/// three whichever end is refusing.
///
/// So the rules are written once and borrowed, rather than mirrored. A mirror would have been
/// one refactor away from losing RFC 3261 §12.2.2's ordering check on the newer side, which is
/// the guard `docs/specs/sip-update.md` §6.1 exists because a new path once sidestepped.
pub(crate) struct EarlyDialog<'a> {
    pub(crate) endpoint: &'a Handle,
    /// The dialog the provisional established (RFC 3261 §12.1.1).
    pub(crate) dialog: &'a mut Dialog,
    /// Where in-dialog requests go, refreshed by a `Contact` on anything that carries one.
    pub(crate) target: &'a mut Target,
    /// Whose turn it is to offer and to answer (RFC 3311 §5, RFC 3264).
    pub(crate) negotiation: &'a mut sipx_sip::update::Negotiation,
    /// Whether the peer's `Allow` listed UPDATE (§4).
    pub(crate) peer_allows: &'a mut bool,
    /// The session already described *and answered*, when there is one.
    ///
    /// Its presence is exactly the difference between an early dialog whose session may be
    /// renegotiated before the call is answered and one whose may not: before the 200 the only
    /// place an answer may travel is a reliable provisional (RFC 3262 §5), and until one has
    /// this side has an offer/answer exchange open.
    pub(crate) early: Option<&'a mut Early>,
}

/// Answer an UPDATE that arrived in an early dialog (RFC 3311 §5.2).
///
/// Returns whether it was one for this dialog. The three refusals are the same three a confirmed
/// dialog gives, and this is the case they were written for: the far end is changing a session
/// that has been described and not yet accepted, which is precisely what a re-INVITE cannot do —
/// a second INVITE inside a transaction that has no final response is not a thing SIP has.
///
/// An offer arriving before this dialog's own offer/answer exchange has closed draws the **500**
/// of §5.2's third rule. Not 491: nothing of ours is outstanding, the peer is simply early, and
/// telling it otherwise would send it into a back-off instead of the retry that will work.
pub(crate) async fn receive(early: EarlyDialog<'_>, incoming: &Incoming) -> Result<bool> {
    if incoming.request.method != Method::Update || !early.dialog.matches(&incoming.request) {
        return Ok(false);
    }

    // §5.2 sends this straight to RFC 3261 §12.2.2, and an early dialog is under §12.2.2 exactly
    // as a confirmed one is. Without it the recorded sequence number rolls *backwards* to
    // whatever the last UPDATE claimed, and a BYE replayed from behind it is then accepted —
    // ending a call that is still running, which is the failure `Call::handle` already refuses
    // in as many words.
    if early.dialog.is_out_of_order(&incoming.request) {
        return respond(
            early.endpoint,
            incoming,
            500,
            "Server Internal Error",
            Vec::new(),
        )
        .await
        .map(|()| true);
    }
    early.dialog.record_remote_cseq(&incoming.request);

    let has_offer = carries_offer(&incoming.request);
    if let Reception::Refuse(refusal) = early.negotiation.receive(has_offer) {
        refuse(early.endpoint, incoming, refusal).await?;
        return Ok(true);
    }

    let mut builder =
        ResponseBuilder::to_request(&incoming.request, crate::call::ok_status(), "OK")?
            .header(
                HeaderName::Contact,
                Bytes::from(contact_for(early.endpoint, early.target.transport)),
            )?
            .header(
                HeaderName::Allow,
                Bytes::from_static(sipx_sip::update::ALLOW.as_bytes()),
            )?;

    if has_offer {
        let answer = match sipx_sdp::parse(&String::from_utf8_lossy(incoming.request.body())) {
            Ok(offer) => match early.early {
                Some(session) => session.reanswer(&offer),
                None => None,
            },
            Err(_) => None,
        };
        let Some(answer) = answer else {
            // §5.2 and `M-8`'s rule together: the change does not happen and the dialog carries
            // on. An early dialog refused this way still rings, and still answers.
            early.negotiation.answered();
            return respond(
                early.endpoint,
                incoming,
                488,
                "Not Acceptable Here",
                vec![(HeaderName::Warning, Bytes::from(warning(early.endpoint)))],
            )
            .await
            .map(|()| true);
        };
        builder = builder
            .header(
                HeaderName::ContentType,
                Bytes::from_static(b"application/sdp"),
            )?
            .body(Bytes::from(answer.to_string_sdp()));
    }

    // §5.1: a target refresh request, in an early dialog as much as a confirmed one. The
    // sequence number was recorded above, with the ordering check that earns the right to.
    early.dialog.refresh_target(&incoming.request.headers);
    *early.target = crate::call::in_dialog_target(
        early.dialog,
        Target::new(incoming.source, incoming.transport),
    );
    *early.peer_allows = sipx_sip::update::peer_allows(&incoming.request.headers);

    let sent = early.endpoint.respond(&incoming.key, builder.build()).await;
    // Cleared whether or not the response got out, for the reason `Call::on_update` gives: a
    // send that will not be retried must not leave the exchange open forever.
    early.negotiation.answered();
    sent?;
    Ok(true)
}

/// Renegotiate an early session from this side (RFC 3311 §5.1).
///
/// Requires that this dialog's first offer/answer exchange has closed — for the answering side
/// that means [`ring_early`](crate::ring_early) put the answer in a reliable provisional, and
/// for the calling side that means one came back in one. Without it this side has an exchange
/// open, RFC 3264 forbids a second, and the far end would answer 491 or 500 and be right to.
pub(crate) async fn offer(early: EarlyDialog<'_>, direction: Direction) -> Result<()> {
    let Some(session) = early.early else {
        return Err(Error::NoEarlySession);
    };
    if !early.negotiation.may_offer() {
        return Err(Error::Rejected {
            status: Refusal::Glare.status(),
            reason: "an offer is already outstanding on this dialog".to_owned(),
        });
    }

    let mut capabilities = session.capabilities.clone();
    capabilities.direction = direction;
    // The version must increase with each modified offer, or the far end cannot tell a changed
    // description from a repeated one.
    capabilities.session_version = u64::from(early.dialog.local_cseq.saturating_add(1));
    let description = crate::call::offer_from(&capabilities);

    let (builder, routes) = request(
        early.endpoint,
        early.dialog,
        early.target,
        Some(&description),
    )?;
    let request = finish(builder, &routes)?;

    early.negotiation.sent_offer();
    let response = send(early.endpoint, request, early.target.clone()).await;
    // Whatever came back closed the exchange: a 2xx carries the answer, and a failure means
    // there will never be one. Leaving the flag set would refuse every later offer of ours.
    early.negotiation.received_answer();
    let response = response?;
    if !response.status.is_success() {
        return Err(rejected(&response));
    }

    early.dialog.refresh_target(&response.headers);
    *early.peer_allows = sipx_sip::update::peer_allows(&response.headers);
    if let Ok(answered) = sipx_sdp::parse(&String::from_utf8_lossy(response.body())) {
        // An answer, not an offer: it says where the far end now wants media, and nothing is
        // owed back for it. Our own port does not move — the peer already has it.
        session.adopt_answer(&answered);
    }
    Ok(())
}

/// Send a bare final response to an in-dialog request, with any headers it must carry.
///
/// Its own function because an early dialog answers several of these — §12.2.2's 500, §5.2's
/// 488 — and a response built inline at each site is a response whose headers drift between
/// them.
async fn respond(
    endpoint: &Handle,
    incoming: &Incoming,
    code: u16,
    reason: &'static str,
    headers: Vec<(HeaderName, Bytes)>,
) -> Result<()> {
    let status =
        StatusCode::new(code).ok_or_else(|| Error::Sdp(format!("status {code} out of range")))?;
    let mut builder = ResponseBuilder::to_request(&incoming.request, status, reason)?;
    for (name, value) in headers {
        builder = builder.header(name, value)?;
    }
    endpoint.respond(&incoming.key, builder.build()).await?;
    Ok(())
}
