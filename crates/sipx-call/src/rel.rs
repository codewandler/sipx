//! Reliable provisional responses in a live call (RFC 3262).
//!
//! The state machine and the header types are in [`sipx_sip::rel`], which has no clock. This is
//! the half that does: sending PRACK when a numbered provisional arrives, and — on the
//! answering side — retransmitting a `180 Ringing` until the caller says it got there.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use sipx_sdp::{Capabilities, Direction, SessionDescription};
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::rel::{self, Numbering, Offered, RAck, RSeq, Reliability};
use sipx_sip::transaction::TransactionKey;
use sipx_sip::update;
use sipx_sip::{HeaderName, Method, Response, StatusCode};
use sipx_transport::{Handle, Incoming, Target};

use crate::call::{Codecs, Early, MediaPolicy};
use crate::dialog::{Dialog, strip_header_params};
use crate::error::{Error, Result};

/// RFC 3261 §17 T1, the round-trip estimate every retransmission schedule is built from.
const T1: Duration = Duration::from_millis(500);

/// §3: "If a reliable provisional response is retransmitted for 64*T1 seconds without reception
/// of a corresponding PRACK, the UAS SHOULD reject the original request."
const GIVE_UP: Duration = Duration::from_secs(32);

/// The body a PRACK must carry, if any (RFC 3262 §5).
///
/// Only one case calls for one: the INVITE carried no offer, so the first reliable provisional
/// had to carry it, and "the UAC ... MUST generate an answer in the PRACK". When the INVITE did
/// offer, whatever SDP comes back in the provisional is the *answer* to it, and putting a
/// second description in the PRACK would start a renegotiation nobody asked for.
#[must_use]
pub fn prack_body(
    invite_offered: bool,
    provisional_body: &[u8],
    capabilities: &Capabilities,
) -> Option<SessionDescription> {
    if invite_offered || provisional_body.is_empty() {
        return None;
    }
    let offer = sipx_sdp::parse(&String::from_utf8_lossy(provisional_body)).ok()?;
    Some(sipx_sdp::answer(&offer, capabilities))
}

/// Whether a provisional response was sent reliably, and its sequence number.
///
/// §4: a `100 Trying` is hop-by-hop, so a `Require: 100rel` on one "MUST be ignored". Checking
/// the status here rather than at the call site is what stops a proxy's `100` from being
/// `PRACK`ed at a UAS that never numbered it.
#[must_use]
pub fn reliable_sequence(response: &Response) -> Option<u32> {
    const TRYING: u16 = 100;
    if response.status.code() <= TRYING || response.status.is_final() {
        return None;
    }
    if !response
        .headers
        .get_all(&HeaderName::Require)
        .any(|header| contains_100rel(&header.value()))
    {
        return None;
    }
    response
        .headers
        .typed::<RSeq>()
        .and_then(std::result::Result::ok)
        .map(|seq| seq.0)
}

fn contains_100rel(value: &[u8]) -> bool {
    value.split(|&b| b == b',').any(|tag| {
        let tag: &[u8] = tag
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .map_or(&[][..], |start| tag.get(start..).unwrap_or_default());
        let end = tag
            .iter()
            .rposition(|b| !b.is_ascii_whitespace())
            .map_or(0, |last| last + 1);
        tag.get(..end)
            .unwrap_or_default()
            .eq_ignore_ascii_case(rel::OPTION_TAG.as_bytes())
    })
}

/// Send the PRACK acknowledging a reliable provisional (RFC 3262 §4).
///
/// It goes inside the dialog the provisional established — which may be a dialog that did not
/// exist a moment ago, since §4 says "the provisional response MUST establish a dialog if one is
/// not yet created". Sending it outside would reach a UAS that has no matching transaction.
pub async fn send_prack(
    endpoint: &Handle,
    dialog: &mut Dialog,
    target: &Target,
    rseq: u32,
    invite_cseq: u32,
    body: Option<SessionDescription>,
) -> Result<()> {
    let (local, remote) = dialog.local_and_remote();
    let cseq = dialog.next_cseq();
    let (uri, routes) = dialog.request_target();
    let ack = RAck {
        rseq,
        cseq: invite_cseq,
        method: Method::Invite.as_bytes().to_vec(),
    };

    let mut builder = RequestBuilder::new(Method::Prack, uri)
        .header(HeaderName::To, Bytes::from(remote))?
        .header(HeaderName::From, Bytes::from(local))?
        .header(HeaderName::CallId, Bytes::from(dialog.id.call_id.clone()))?
        .cseq(cseq, &Method::Prack)?
        .header(HeaderName::RAck, Bytes::from(ack.to_string()))?
        .max_forwards(70);
    if let Some(answer) = body {
        builder = builder
            .header(
                HeaderName::ContentType,
                Bytes::from_static(b"application/sdp"),
            )?
            .body(Bytes::from(answer.to_string_sdp()));
    }

    let request = crate::call::add_routes(builder, &routes)?.build();
    let mut responses = endpoint.send(request, target.clone()).await?;
    // §3: a matching PRACK "MUST be responded to with a 2xx". A failure here is worth
    // surfacing rather than swallowing — a 481 means the UAS has no record of the provisional
    // we just acknowledged, so the two sides disagree about what has happened.
    match responses.final_response().await {
        Some(response) if response.status.is_success() => Ok(()),
        Some(response) => Err(Error::Rejected {
            status: response.status.code(),
            reason: String::from_utf8_lossy(&response.reason).into_owned(),
        }),
        None => Err(Error::NoResponse),
    }
}

/// An invitation that has been rung but not yet answered.
///
/// Holding this is what makes a reliable `180` possible at all: the response has to be
/// retransmitted until a PRACK arrives, and something has to own the sequence number and the
/// early dialog's tag in the meantime.
#[derive(Debug)]
pub struct Ringing {
    endpoint: Handle,
    tag: String,
    invite_cseq: u32,
    numbering: Numbering,
    reliable: bool,
    stop: Option<Arc<tokio::sync::Notify>>,
    acknowledged: bool,
    /// The early dialog the provisional created (RFC 3261 §12.1.1).
    ///
    /// `None` only when the INVITE carried no usable `Contact`, which is a caller we could not
    /// address anyway. It is held here rather than rebuilt at answer time because an UPDATE
    /// arriving in the meantime numbers itself against it, and a dialog rebuilt afterwards
    /// would have forgotten that.
    dialog: Option<Dialog>,
    /// Where in-dialog requests go while the invitation is still ringing.
    target: Target,
    /// Whose turn it is to offer and to answer (RFC 3311 §5, RFC 3264).
    negotiation: update::Negotiation,
    /// Whether the caller's `Allow` listed UPDATE (RFC 3311 §4).
    peer_allows_update: bool,
    /// The session this side answered in the provisional, when it answered one.
    ///
    /// Its presence is exactly the difference between a dialog whose session may be
    /// renegotiated before it is answered and one whose may not — see [`ring_early`].
    early: Option<Early>,
}

impl Ringing {
    /// The `To` tag this side chose, which the eventual 200 must reuse.
    ///
    /// A provisional that establishes a dialog has already told the caller what the remote tag
    /// is (RFC 3261 §12.1.1). Answering later with a *different* tag creates a second dialog,
    /// and the caller ACKs the one it knows about while this side waits for an ACK to the other.
    #[must_use]
    pub fn tag(&self) -> &str {
        &self.tag
    }

    /// Whether the provisional was sent reliably.
    #[must_use]
    pub fn is_reliable(&self) -> bool {
        self.reliable
    }

    /// Whether the caller has acknowledged it.
    #[must_use]
    pub fn is_acknowledged(&self) -> bool {
        self.acknowledged || !self.reliable
    }

    /// Whether the caller's `Allow` listed UPDATE (RFC 3311 §4).
    #[must_use]
    pub fn peer_allows_update(&self) -> bool {
        self.peer_allows_update
    }

    /// Whether the session was described *and answered* before the call was accepted.
    ///
    /// True only after [`ring_early`]. It is what makes an offer-carrying UPDATE legal in this
    /// dialog: RFC 3311 §5.1 will not let one out while an offer/answer exchange is open, and
    /// before the 200 the only way to close one is RFC 3262 §5's answer in a reliable
    /// provisional.
    #[must_use]
    pub fn has_early_session(&self) -> bool {
        self.early.is_some()
    }

    /// Hand the early session over to the [`Call`](crate::Call) that is taking its place.
    ///
    /// Empties this ringing rather than consuming it, because it still owns the retransmission
    /// of the provisional and must go on owning it until it is dropped.
    pub(crate) fn take_early(&mut self) -> Result<(Early, Dialog, update::Negotiation, bool)> {
        let early = self.early.take().ok_or(Error::NoEarlySession)?;
        let dialog = self.dialog.take().ok_or(Error::NoDialog)?;
        Ok((early, dialog, self.negotiation, self.peer_allows_update))
    }

    /// The early dialog's mutable parts, borrowed for one UPDATE.
    ///
    /// `None` when the INVITE carried no usable `Contact` and no dialog was ever built, which is
    /// a peer there is nothing to answer *to*.
    fn early_dialog(&mut self) -> Option<crate::update::EarlyDialog<'_>> {
        Some(crate::update::EarlyDialog {
            endpoint: &self.endpoint,
            dialog: self.dialog.as_mut()?,
            target: &mut self.target,
            negotiation: &mut self.negotiation,
            peer_allows: &mut self.peer_allows_update,
            early: self.early.as_mut(),
        })
    }

    /// Answer an UPDATE that arrived in the early dialog (RFC 3311 §5.2).
    ///
    /// Returns whether it was one for this dialog. The rules are the same code the *calling*
    /// side runs from [`Dialing::on_update`](crate::Dialing::on_update): §5.1 makes UPDATE
    /// something either end may send, so a second copy of §5.2 here would be a second place for
    /// it to drift.
    ///
    /// An offer arriving before this side has answered the INVITE's own — that is, after
    /// [`ring`] rather than [`ring_early`] — draws the **500** of §5.2's third rule. Not 491:
    /// nothing of ours is outstanding, the peer is simply early, and telling it otherwise would
    /// send it into a back-off instead of the retry that will work.
    pub async fn on_update(&mut self, incoming: &Incoming) -> Result<bool> {
        let Some(early) = self.early_dialog() else {
            return Ok(false);
        };
        crate::update::receive(early, incoming).await
    }

    /// Renegotiate the early session from this side (RFC 3311 §5.1).
    ///
    /// Requires [`ring_early`]: without an answer already given to the INVITE's offer this side
    /// owes one, and RFC 3264 forbids a second offer while one is open — the far end would
    /// answer 500 and be right to.
    pub async fn update(&mut self, direction: Direction) -> Result<()> {
        // `NoDialog` for a missing early session as well as for a missing dialog, which is this
        // method's existing contract. `crate::update::offer` distinguishes the two, and the
        // caller's handle takes the sharper error; changing what an application already matches
        // on is not something a story about the *other* role should do on its way past.
        if self.early.is_none() {
            return Err(Error::NoDialog);
        }
        let Some(early) = self.early_dialog() else {
            return Err(Error::NoDialog);
        };
        crate::update::offer(early, direction).await
    }

    /// Handle an in-dialog PRACK. Returns whether it was one for this ringing.
    ///
    /// §3: a PRACK that matches is answered 2xx and stops the retransmissions; one that matches
    /// nothing "MUST" be answered 481. Answering 481 matters more than it looks — it tells a
    /// caller that acknowledged something we never sent that the two sides disagree, instead of
    /// leaving its PRACK transaction to time out looking like a lost packet.
    pub async fn on_prack(&mut self, incoming: &Incoming) -> Result<bool> {
        if incoming.request.method != Method::Prack {
            return Ok(false);
        }
        let ack = incoming
            .request
            .headers
            .typed::<RAck>()
            .and_then(std::result::Result::ok);

        let matched = ack.is_some_and(|ack| {
            self.numbering
                .acknowledge(&ack, self.invite_cseq, Method::Invite.as_bytes())
        });

        let (status, reason) = if matched {
            (200, "OK")
        } else {
            (481, "Call/Transaction Does Not Exist")
        };
        let code = StatusCode::new(status)
            .ok_or_else(|| Error::Sdp("unreachable: literal status".to_owned()))?;
        let response = ResponseBuilder::to_request(&incoming.request, code, reason)?.build();
        self.endpoint.respond(&incoming.key, response).await?;

        if matched {
            self.acknowledged = true;
            if let Some(stop) = self.stop.take() {
                stop.notify_waiters();
            }
        }
        Ok(matched)
    }
}

impl Drop for Ringing {
    fn drop(&mut self) {
        // Retransmissions outlive this value otherwise, and would go on resending a `180` for a
        // call that has since been answered or abandoned.
        if let Some(stop) = self.stop.take() {
            stop.notify_waiters();
        }
    }
}

/// Ring: send a provisional response, reliably if RFC 3262 says to.
///
/// `enabled` is local policy for 100rel. A caller that put `100rel` in `Require` and is told no
/// gets a `420 Bad Extension` naming the tag (§3) and this returns an error — refusing plainly
/// beats accepting and then never numbering anything, which the caller cannot tell from a dead
/// network.
pub async fn ring(
    endpoint: &Handle,
    incoming: &Incoming,
    status: u16,
    reason: &'static str,
    enabled: bool,
) -> Result<Ringing> {
    ring_with(endpoint, incoming, status, reason, enabled, None).await
}

/// Ring, and answer the INVITE's offer in the provisional (RFC 3262 §5 + RFC 3311 §4).
///
/// This is what makes an early dialog *renegotiable*. RFC 3311 §5.1 will not let an UPDATE
/// carry an offer while an offer/answer exchange is open, so a session described in the INVITE
/// cannot be changed before the 200 unless its answer has already gone back — and before the
/// 200 there is exactly one place to put an answer: a reliable provisional response.
///
/// 100rel is therefore not optional here and there is no flag to switch it off. RFC 3262 §5
/// forbids an answer in an unreliable provisional outright, and one sent anyway can be lost
/// without either side noticing, leaving them disagreeing about which description is in force.
/// A caller that did not offer 100rel gets an error and should fall back to [`ring`].
///
/// The media port is bound now, and the [`Call`](crate::Call) that
/// [`answer_early`](crate::answer_early) builds takes it over: the answer has already told the
/// far end where to send, and binding a second port would make the 200 contradict the 183.
///
/// Answers from the default codec set, [`Codecs::G711`]. [`ring_early_with`] takes a selection,
/// and it has to be made *here* rather than at [`crate::answer_early`]: the answer goes out in
/// this provisional, so by the time the 200 is built the codec has been agreed for some time.
pub async fn ring_early(
    endpoint: &Handle,
    incoming: &Incoming,
    status: u16,
    reason: &'static str,
    media_address: IpAddr,
) -> Result<Ringing> {
    ring_early_with(
        endpoint,
        incoming,
        status,
        reason,
        media_address,
        Codecs::default(),
    )
    .await
}

/// [`ring_early`], from a chosen codec set rather than the default one (`M-30`).
///
/// The [`Call`](crate::Call) that [`crate::answer_early`] builds inherits `codecs`, so an UPDATE
/// arriving before the 200 — the whole reason this path exists — is answered from the same set the
/// 183 answered from rather than from the default one.
pub async fn ring_early_with(
    endpoint: &Handle,
    incoming: &Incoming,
    status: u16,
    reason: &'static str,
    media_address: IpAddr,
    codecs: Codecs,
) -> Result<Ringing> {
    ring_early_with_policy(
        endpoint,
        incoming,
        status,
        reason,
        media_address,
        MediaPolicy::default().with_codecs(codecs),
    )
    .await
}

/// [`ring_early`], using one coherent codec and ICE policy.
///
/// ICE has to be selected here because the answer and its candidates leave in the provisional;
/// [`crate::answer_early`] only confirms the already-completed exchange.
pub async fn ring_early_with_policy(
    endpoint: &Handle,
    incoming: &Incoming,
    status: u16,
    reason: &'static str,
    media_address: IpAddr,
    policy: MediaPolicy,
) -> Result<Ringing> {
    if !Offered::in_request(&incoming.request).supported {
        // Not a refusal of the call — the caller can still be rung the ordinary way. It is a
        // refusal to put an answer somewhere it may be silently lost.
        return Err(Error::Rejected {
            status: 421,
            reason: "the caller did not offer 100rel, so no answer may go in a provisional"
                .to_owned(),
        });
    }
    let offer = sipx_sdp::parse(&String::from_utf8_lossy(incoming.request.body()))
        .map_err(|error| Error::Sdp(error.to_string()))?;
    let settled = Early::settle(
        media_address,
        incoming.transport.is_secure(),
        &offer,
        policy,
    )
    .await?;
    ring_with(endpoint, incoming, status, reason, true, Some(settled)).await
}

async fn ring_with(
    endpoint: &Handle,
    incoming: &Incoming,
    status: u16,
    reason: &'static str,
    enabled: bool,
    early: Option<(Early, SessionDescription)>,
) -> Result<Ringing> {
    let offered = Offered::in_request(&incoming.request);
    let decision = rel::reliability(offered, enabled);

    if decision == Reliability::Refuse {
        return refuse_bad_extension(endpoint, incoming).await;
    }

    let tag = crate::call::token();
    let invite_cseq = incoming
        .request
        .headers
        .typed::<sipx_sip::CSeq>()
        .and_then(std::result::Result::ok)
        .map_or(1, |cseq| cseq.sequence);

    // §3: "The value of the header field for the first reliable provisional response ... MUST
    // be between 1 and 2**31 - 1. It is RECOMMENDED that it be chosen uniformly in this range."
    // Uniform rather than sequential because the numbering is a per-transaction secret: a
    // predictable one lets an off-path attacker forge a PRACK and stop the retransmissions.
    let mut numbering = Numbering::starting_at({
        use rand::Rng as _;
        rand::rng().random_range(1..=rel::MAX_FIRST_RSEQ)
    });

    let code = StatusCode::new(status)
        .ok_or_else(|| Error::Sdp(format!("status {status} out of range")))?;
    let to_with_tag = {
        let existing = incoming
            .request
            .headers
            .value(&HeaderName::To)
            .map(|value| String::from_utf8_lossy(&value).into_owned())
            .unwrap_or_default();
        format!("{};tag={tag}", strip_header_params(&existing))
    };

    let mut builder = ResponseBuilder::to_request(&incoming.request, code, reason)?
        .set_header(&HeaderName::To, Bytes::from(to_with_tag))?
        .header(
            HeaderName::Contact,
            Bytes::from(crate::call::contact_for(endpoint, incoming.transport)),
        )?
        // RFC 3311 §4: a reliable provisional carrying SDP "SHOULD contain an Allow header
        // field that lists the UPDATE method", which is the far end's permission to renegotiate
        // the session this response just answered. It goes on every provisional rather than
        // only that one, because a peer that learns it earlier can act on it earlier and
        // nothing is claimed that is not true.
        .header(
            HeaderName::Allow,
            Bytes::from_static(update::ALLOW.as_bytes()),
        )?;

    let reliable = decision != Reliability::Forbidden;
    if reliable {
        let allocated = numbering
            .allocate()
            .ok_or_else(|| Error::Sdp("unreachable: first allocation".to_owned()))?;
        builder = builder
            .header(HeaderName::Require, Bytes::from_static(b"100rel"))?
            .header(HeaderName::RSeq, Bytes::from(allocated.to_string()))?;
    }

    // The answer, when there is one. Guarded by `reliable` because RFC 3262 §5 permits an
    // answer only in a reliable provisional; `ring_early` has already refused the case where
    // that cannot be met, so reaching here with an unreliable response and a description would
    // be a bug rather than a peer's doing.
    let early = match early {
        Some((settled, answer)) if reliable => {
            builder = builder
                .header(
                    HeaderName::ContentType,
                    Bytes::from_static(b"application/sdp"),
                )?
                .body(Bytes::from(answer.to_string_sdp()));
            Some(settled)
        }
        _ => None,
    };

    let response = builder.build();
    endpoint.respond(&incoming.key, response.clone()).await?;

    let stop = reliable.then(|| {
        let stop = Arc::new(tokio::sync::Notify::new());
        tokio::spawn(retransmit_until_pracked(
            endpoint.clone(),
            incoming.key.clone(),
            response,
            Arc::clone(&stop),
        ));
        stop
    });

    // The offer/answer state the early dialog starts in. With an answer already sent nothing is
    // outstanding either way, so an UPDATE carrying an offer is legal; without one this side
    // owes an answer to the INVITE, and RFC 3311 §5.2's third rule refuses such an UPDATE with
    // a 500 until the 200 settles it.
    let negotiation = if early.is_none() && crate::update::carries_offer(&incoming.request) {
        update::Negotiation::owing()
    } else {
        update::Negotiation::idle()
    };

    let dialog = Dialog::from_request(&incoming.request, &tag);
    let target = dialog.as_ref().map_or_else(
        || Target::new(incoming.source, incoming.transport),
        |dialog| {
            crate::call::in_dialog_target(dialog, Target::new(incoming.source, incoming.transport))
        },
    );

    Ok(Ringing {
        endpoint: endpoint.clone(),
        tag,
        invite_cseq,
        numbering,
        reliable,
        stop,
        acknowledged: false,
        dialog,
        target,
        negotiation,
        peer_allows_update: update::peer_allows(&incoming.request.headers),
        early,
    })
}

/// Refuse an invitation that requires 100rel from a side that has it switched off (§3).
///
/// The `Unsupported` naming the tag is what makes this actionable: without it the caller learns
/// only that it failed, and a caller left waiting for an `RSeq` that will never come cannot tell
/// that from a dead network.
async fn refuse_bad_extension(endpoint: &Handle, incoming: &Incoming) -> Result<Ringing> {
    const BAD_EXTENSION: u16 = 420;
    let code = StatusCode::new(BAD_EXTENSION)
        .ok_or_else(|| Error::Sdp("unreachable: literal status".to_owned()))?;
    let refusal = ResponseBuilder::to_request(&incoming.request, code, "Bad Extension")?
        .header(HeaderName::Unsupported, Bytes::from_static(b"100rel"))?
        .build();
    endpoint.respond(&incoming.key, refusal).await?;
    Err(Error::Rejected {
        status: BAD_EXTENSION,
        reason: "Bad Extension".to_owned(),
    })
}

/// Resend a reliable provisional on the RFC 3262 §3 schedule until it is acknowledged.
///
/// The interval "starts at T1 seconds and doubles for each retransmission" — and, unlike a 2xx,
/// **does not cap at T2**. The RFC explains why: ACK retransmissions are triggered by receiving
/// a 2xx, but PRACK is sent once and independently of further 1xx, so a fast repeat buys
/// nothing after the first few and only adds traffic.
async fn retransmit_until_pracked(
    endpoint: Handle,
    key: TransactionKey,
    response: Response,
    stop: Arc<tokio::sync::Notify>,
) {
    let deadline = tokio::time::Instant::now() + GIVE_UP;
    let mut interval = T1;
    loop {
        let wake = tokio::time::Instant::now() + interval;
        if wake >= deadline {
            return;
        }
        tokio::select! {
            () = stop.notified() => return,
            () = tokio::time::sleep_until(wake) => {}
        }
        if endpoint.respond(&key, response.clone()).await.is_err() {
            return;
        }
        interval = interval.saturating_mul(2);
    }
}
