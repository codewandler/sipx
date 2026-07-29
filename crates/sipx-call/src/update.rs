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
use sipx_sdp::SessionDescription;
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::update::Refusal;
use sipx_sip::{HeaderName, Method, Request, StatusCode};
use sipx_transport::{Handle, Incoming, Target};

use crate::call::{add_routes, contact_for};
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
