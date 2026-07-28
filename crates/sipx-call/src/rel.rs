//! Reliable provisional responses in a live call (RFC 3262).
//!
//! The state machine and the header types are in [`sipx_sip::rel`], which has no clock. This is
//! the half that does: sending PRACK when a numbered provisional arrives, and — on the
//! answering side — retransmitting a `180 Ringing` until the caller says it got there.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use sipx_sdp::{Capabilities, SessionDescription};
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::rel::{self, Numbering, Offered, RAck, RSeq, Reliability};
use sipx_sip::transaction::TransactionKey;
use sipx_sip::{HeaderName, Method, Response, StatusCode};
use sipx_transport::{Handle, Incoming, Target};

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
    let offered = Offered::in_request(&incoming.request);
    let decision = rel::reliability(offered, enabled);

    if decision == Reliability::Refuse {
        const BAD_EXTENSION: u16 = 420;
        let code = StatusCode::new(BAD_EXTENSION)
            .ok_or_else(|| Error::Sdp("unreachable: literal status".to_owned()))?;
        let refusal = ResponseBuilder::to_request(&incoming.request, code, "Bad Extension")?
            .header(HeaderName::Unsupported, Bytes::from_static(b"100rel"))?
            .build();
        endpoint.respond(&incoming.key, refusal).await?;
        return Err(Error::Rejected {
            status: BAD_EXTENSION,
            reason: "Bad Extension".to_owned(),
        });
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

    Ok(Ringing {
        endpoint: endpoint.clone(),
        tag,
        invite_cseq,
        numbering,
        reliable,
        stop,
        acknowledged: false,
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
