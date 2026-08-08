//! Two dialogs coupled with sipx off the media path (RFC 7092 §3.1.3).
//!
//! [`Coupling`](super::Coupling) owns two [`Call`]s, and a `Call` binds and advertises a local
//! media endpoint whether or not a bridge forwards anything. That is §3.2.3 — media termination —
//! and leaving the bridge off does not make it anything else. This module is the other role: the
//! coupling owns two *dialogs*, no media session exists on either leg, and the descriptions the
//! endpoints wrote are put in front of each other with only their `o=` line replaced. The
//! endpoints therefore address each other directly and sipx is never on the media path.
//!
//! The lifecycle is deliberately not a second one. Glare, CANCEL, BYE and final-failure mapping
//! all run through the same [`CouplingState`] the media-terminating role uses; what differs is
//! only what the two legs are made of.
//!
//! What this role does not do, and refuses rather than half-does: it does not originate a
//! description of its own, so an offerless initial INVITE and an offerless re-INVITE are refused
//! `488`, and it does not advertise `100rel`, so no peer may put an offer in a reliable
//! provisional. Those carriers need a description sipx would have to author, and authoring one
//! means describing a media endpoint this role does not have.

use std::collections::VecDeque;
use std::net::IpAddr;
use std::time::Duration;

use bytes::Bytes;
use sipx_sdp::relay::DescriptionRelay;
use sipx_sdp::session::Origin;
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::headers::CSeq;
use sipx_sip::{HeaderName, Method, Request, Response, StatusCode, Uri};
use sipx_transport::{Handle, Incoming, Target};
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::{
    CouplingEnd, CouplingState, DEFERRED_CAPACITY, FailureAction, Leg, OfferAction, OfferAxis,
};
use crate::call::{
    add_routes, build_ack, contact_for, in_dialog_target, normal_clearing_reason,
    reack_retransmitted_2xx, sleep_until, withdraw,
};
use crate::dialog::Dialog;
use crate::{Call, Calls, Error, Invitation, Result};

/// RFC 3261 §17.2.1 timers for retransmitting an INVITE's 2xx until its ACK.
const T1: Duration = Duration::from_millis(500);
const T2: Duration = Duration::from_secs(4);
const TIMER_H: Duration = Duration::from_secs(32);

/// How an off-media coupling places its target leg.
///
/// Deliberately not [`DialOptions`](crate::DialOptions): every media field on that type would be
/// a lie here. What is left is the identity this side signals with and the identity it puts on
/// the descriptions it relays.
#[derive(Debug, Clone)]
pub struct OffMediaOptions {
    /// Our own address of record, as it appears in `From`.
    pub from: String,
    /// The address written in the `o=` lines this coupling emits.
    ///
    /// RFC 8866 §5.2 makes the origin address the identity of *whoever created the description*,
    /// explicitly not a destination for media — that is the `c=` line, which stays the far
    /// endpoint's own. Nothing is bound on this address.
    pub origin_address: IpAddr,
    /// How long to wait for the target's final response before withdrawing the invitation.
    pub timeout: Option<Duration>,
    /// How long withdrawing that invitation may wait for its protocol completion events.
    pub cancellation_timeout: Duration,
}

impl OffMediaOptions {
    /// Options with no answer deadline and the ordinary cancellation allowance.
    #[must_use]
    pub fn new(from: impl Into<String>, origin_address: IpAddr) -> Self {
        Self {
            from: from.into(),
            origin_address,
            timeout: None,
            cancellation_timeout: Duration::from_secs(2),
        }
    }

    /// Give up on the target's final response after this long.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

/// A 2xx being retransmitted until its ACK arrives (RFC 3261 §13.3.1.4).
#[derive(Debug)]
struct Acknowledging {
    key: sipx_sip::transaction::TransactionKey,
    response: Response,
    sequence: u32,
    interval: Duration,
    next: Instant,
    deadline: Instant,
}

/// One dialog of an off-media coupling: no media session, and no `Call` to hold one.
#[derive(Debug)]
struct OffMediaLeg {
    dialog: Dialog,
    target: Target,
    inbox: mpsc::Receiver<Incoming>,
    /// The descriptions this coupling emits **into** this dialog, and their version sequence.
    relay: DescriptionRelay,
    acknowledging: Option<Acknowledging>,
    deferred: VecDeque<Incoming>,
    ended: bool,
}

impl OffMediaLeg {
    fn retransmit_at(&self) -> Option<Instant> {
        self.acknowledging.as_ref().map(|pending| pending.next)
    }

    /// Whether this ACK settles the outstanding 2xx.
    fn acknowledged_by(&mut self, request: &Request) -> bool {
        let Some(pending) = &self.acknowledging else {
            return false;
        };
        let matched = sequence_of(request, &Method::Ack) == Some(pending.sequence);
        if matched {
            self.acknowledging = None;
        }
        matched
    }

    /// Resend the 2xx, or report that Timer H expired without an ACK.
    async fn retransmit(&mut self, endpoint: &Handle) -> Result<()> {
        let Some(pending) = &mut self.acknowledging else {
            return Ok(());
        };
        let now = Instant::now();
        if now >= pending.deadline {
            self.acknowledging = None;
            return Err(Error::NoResponse);
        }
        pending.interval = (pending.interval * 2).min(T2);
        pending.next = now + pending.interval;
        let (key, response) = (pending.key.clone(), pending.response.clone());
        endpoint.respond(&key, response).await?;
        Ok(())
    }
}

/// Two dialogs driven as one call while sipx stays off the media path.
///
/// The two endpoints keep their own media addresses, ports, payload types and keys: this owner
/// relays their descriptions and never appears in one. Created by [`Self::dial`], driven by
/// [`Self::run`].
#[derive(Debug)]
pub struct OffMediaCoupling {
    endpoint: Handle,
    state: CouplingState,
    one: OffMediaLeg,
    two: OffMediaLeg,
}

impl OffMediaCoupling {
    /// Consume an inbound invitation and create its relayed target leg, off the media path.
    ///
    /// The source endpoint's own description is what the target is offered, and the target's own
    /// description is what the source is answered with. Target selection stays above this crate,
    /// exactly as it does for the media-terminating role.
    ///
    /// Cancellation remains this object's responsibility for as long as it holds the invitation:
    /// a CANCEL that arrives while the target INVITE is outstanding withdraws that INVITE,
    /// including the case where its 2xx crossed the CANCEL.
    pub async fn dial(
        invitation: Invitation,
        calls: &Calls,
        endpoint: &Handle,
        target: Target,
        to: &Uri,
        options: &OffMediaOptions,
    ) -> Result<Self> {
        let mut two_relay = DescriptionRelay::new(fresh_origin(options.origin_address));
        let relayed = match source_offer(invitation.request())
            .and_then(|offer| two_relay.relay(offer).map_err(Error::Relay))
        {
            Ok(relayed) => relayed,
            Err(error) => {
                invitation
                    .refuse(endpoint, 488, "Not Acceptable Here")
                    .await?;
                return Err(error);
            }
        };

        let invitation = invitation.into_coupling();
        let mut state = CouplingState::new();
        let _relay = state.begin_offer(Leg::One, OfferAxis::InitialInvite);
        let cancellation = invitation.cancellation();

        let invite = offer_invite(endpoint, &target, to, options, &relayed)?;
        let mut responses = endpoint.send(invite.clone(), target.clone()).await?;
        let response = tokio::select! {
            response = await_final(&mut responses, options.timeout) => response,
            () = cancellation.cancelled() => {
                let _cleanup = withdraw(
                    endpoint,
                    &invite,
                    target.clone(),
                    &mut responses,
                    &normal_clearing_reason(),
                    options.cancellation_timeout,
                )
                .await;
                return Err(Error::InvitationCancelled);
            }
        };
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                invitation
                    .refuse(endpoint, 503, "Service Unavailable")
                    .await?;
                return Err(error);
            }
        };
        if !response.status.is_success() {
            let status = response.status.code();
            let reason = String::from_utf8_lossy(&response.reason).into_owned();
            // The peer leg has no dialog yet, so `C-1`'s lifecycle table maps this onto the
            // inbound INVITE's own final response rather than onto a BYE.
            let FailureAction::RejectPeer { status } = state.final_failure(Leg::Two, status) else {
                return Err(Error::Rejected { status, reason });
            };
            invitation.refuse(endpoint, status, reason.clone()).await?;
            return Err(Error::Rejected { status, reason });
        }

        let mut two = match confirm_target(
            endpoint, calls, &invite, &response, target, responses, two_relay,
        )
        .await
        {
            Ok(two) => two,
            Err(error) => {
                invitation
                    .refuse(endpoint, 503, "Service Unavailable")
                    .await?;
                return Err(error);
            }
        };

        let one_relay = DescriptionRelay::new(fresh_origin(options.origin_address));
        let one = accept_source(endpoint, invitation, one_relay, response.body(), &mut two).await?;
        let _completed = state.complete(Leg::One);
        state.confirm(Leg::Two);
        state.confirm(Leg::One);
        Ok(Self {
            endpoint: endpoint.clone(),
            state,
            one,
            two,
        })
    }

    /// The shared offer/answer and lifecycle policy. The same type the media-terminating role
    /// uses, because the off-media role is not a second state machine.
    #[must_use]
    pub fn state(&self) -> &CouplingState {
        &self.state
    }

    /// The two dialogs this coupling owns, for observation and route release.
    #[must_use]
    pub fn dialogs(&self) -> (&Dialog, &Dialog) {
        (&self.one.dialog, &self.two.dialog)
    }

    /// Drive both routed inboxes until either dialog ends.
    ///
    /// A BYE is answered on the leg it arrived on and then sent on the peer, an offer is mapped
    /// and relayed on the axis it arrived on, and a closed inbox ends the peer rather than
    /// orphaning it — the same policy the media-terminating driver applies.
    pub async fn run(&mut self) -> Result<CouplingEnd> {
        loop {
            if let Some(request) = self.one.deferred.pop_front() {
                if let Some(end) = self.handle(Leg::One, request).await? {
                    return Ok(end);
                }
                continue;
            }
            if let Some(request) = self.two.deferred.pop_front() {
                if let Some(end) = self.handle(Leg::Two, request).await? {
                    return Ok(end);
                }
                continue;
            }
            let one_retransmit = self.one.retransmit_at();
            let two_retransmit = self.two.retransmit_at();
            let (leg, request) = tokio::select! {
                request = self.one.inbox.recv() => (Leg::One, request),
                request = self.two.inbox.recv() => (Leg::Two, request),
                () = sleep_until(one_retransmit) => {
                    self.one.retransmit(&self.endpoint).await?;
                    continue;
                }
                () = sleep_until(two_retransmit) => {
                    self.two.retransmit(&self.endpoint).await?;
                    continue;
                }
            };
            let Some(request) = request else {
                let peer = leg.peer();
                let endpoint = self.endpoint.clone();
                end_leg(&endpoint, self.leg_mut(peer)).await;
                return Ok(CouplingEnd::InboxClosed(leg));
            };
            if let Some(end) = self.handle(leg, request).await? {
                return Ok(end);
            }
        }
    }

    /// End both dialogs, whichever of them is still alive.
    pub async fn hang_up(&mut self) -> Result<()> {
        let endpoint = self.endpoint.clone();
        end_leg(&endpoint, &mut self.one).await;
        end_leg(&endpoint, &mut self.two).await;
        Ok(())
    }

    async fn handle(&mut self, leg: Leg, incoming: Incoming) -> Result<Option<CouplingEnd>> {
        if incoming.request.method == Method::Ack {
            // An ACK is never responded to, so a stray one is dropped rather than refused.
            if self.leg(leg).dialog.matches(&incoming.request) {
                let _settled = self.leg_mut(leg).acknowledged_by(&incoming.request);
            }
            return Ok(None);
        }
        if !self.leg(leg).dialog.matches(&incoming.request) {
            Call::refuse_with(
                &self.endpoint,
                &incoming,
                481,
                "Call/Transaction Does Not Exist",
            )
            .await?;
            return Ok(None);
        }
        if self.leg(leg).dialog.is_out_of_order(&incoming.request) {
            Call::refuse_with(&self.endpoint, &incoming, 500, "Server Internal Error").await?;
            return Ok(None);
        }
        match incoming.request.method {
            Method::Bye => self.relay_bye(leg, &incoming).await,
            Method::Invite | Method::Update => self.relay_offer(leg, incoming).await,
            _ => {
                Call::refuse_with(&self.endpoint, &incoming, 405, "Method Not Allowed").await?;
                Ok(None)
            }
        }
    }

    async fn relay_bye(&mut self, leg: Leg, incoming: &Incoming) -> Result<Option<CouplingEnd>> {
        self.leg_mut(leg)
            .dialog
            .record_remote_cseq(&incoming.request);
        respond(&self.endpoint, incoming, 200, "OK", None).await?;
        self.leg_mut(leg).ended = true;
        let endpoint = self.endpoint.clone();
        end_leg(&endpoint, self.leg_mut(leg.peer())).await;
        Ok(Some(CouplingEnd::Bye(leg)))
    }

    async fn relay_offer(&mut self, leg: Leg, incoming: Incoming) -> Result<Option<CouplingEnd>> {
        let axis = match incoming.request.method {
            Method::Update => OfferAxis::Update,
            _ => OfferAxis::Reinvite,
        };
        if !crate::update::carries_offer(&incoming.request) {
            // An UPDATE with no offer changes nothing about the session (RFC 3311 §5.1) and is
            // answered here. A re-INVITE with no offer asks this side to originate a
            // description, which is the one thing this role cannot do.
            if axis == OfferAxis::Update {
                self.leg_mut(leg)
                    .dialog
                    .record_remote_cseq(&incoming.request);
                respond(&self.endpoint, &incoming, 200, "OK", None).await?;
            } else {
                Call::refuse_with(&self.endpoint, &incoming, 488, "Not Acceptable Here").await?;
            }
            return Ok(None);
        }

        // Mapped before any coupling state opens and before the peer leg is told anything: a
        // description this side cannot map leaves both dialogs exactly as they were.
        let peer = leg.peer();
        let Ok(mapped) = description(incoming.request.body())
            .and_then(|offer| self.leg_mut(peer).relay.relay(offer).map_err(Error::Relay))
        else {
            Call::refuse_with(&self.endpoint, &incoming, 488, "Not Acceptable Here").await?;
            return Ok(None);
        };

        match self.state.begin_offer(leg, axis) {
            OfferAction::Refuse { status } => {
                let reason = if status == 491 {
                    "Request Pending"
                } else {
                    "Server Internal Error"
                };
                Call::refuse_with(&self.endpoint, &incoming, status, reason).await?;
                return Ok(None);
            }
            OfferAction::Relay { .. } => {}
        }

        let Self {
            endpoint,
            state,
            one,
            two,
        } = self;
        let (source, far) = match leg {
            Leg::One => (&mut *one, &mut *two),
            Leg::Two => (&mut *two, &mut *one),
        };
        let (relayed, inbox_closed) = relay_to(
            endpoint,
            state,
            peer,
            far,
            &incoming.request.method,
            &mapped,
        )
        .await?;

        let response = match relayed {
            Ok(response) => response,
            Err(error) => {
                let _settled = state.fail(leg);
                let Error::Rejected { status, reason } = error else {
                    return Err(error);
                };
                Call::refuse_with(endpoint, &incoming, status, reason).await?;
                return Ok(None);
            }
        };

        let answered = description(response.body())
            .and_then(|answer| source.relay.relay(answer).map_err(Error::Relay));
        let answer = match answered {
            Ok(answer) => answer,
            Err(error) => {
                // The far leg has already answered and been acknowledged, so the two dialogs now
                // disagree about the session. Nothing here can repair that: the error is returned
                // so the owner ends the coupling rather than driving on with a split view.
                let _settled = state.fail(leg);
                Call::refuse_with(endpoint, &incoming, 488, "Not Acceptable Here").await?;
                return Err(error);
            }
        };
        source.dialog.record_remote_cseq(&incoming.request);
        let accepted = accept_in_dialog(endpoint, &incoming, &source.target, &answer)?;
        endpoint.respond(&incoming.key, accepted.clone()).await?;
        if incoming.request.method == Method::Invite {
            let now = Instant::now();
            source.acknowledging = Some(Acknowledging {
                key: incoming.key.clone(),
                response: accepted,
                sequence: sequence_of(&incoming.request, &Method::Invite).unwrap_or_default(),
                interval: T1,
                next: now + T1,
                deadline: now + TIMER_H,
            });
        }
        let _settled = state.complete(leg);

        if inbox_closed {
            let endpoint = endpoint.clone();
            end_leg(&endpoint, source).await;
            return Ok(Some(CouplingEnd::InboxClosed(peer)));
        }
        Ok(None)
    }

    fn leg(&self, leg: Leg) -> &OffMediaLeg {
        match leg {
            Leg::One => &self.one,
            Leg::Two => &self.two,
        }
    }

    fn leg_mut(&mut self, leg: Leg) -> &mut OffMediaLeg {
        match leg {
            Leg::One => &mut self.one,
            Leg::Two => &mut self.two,
        }
    }
}

/// Take ownership of the confirmed target dialog: register its inbox and acknowledge its 2xx.
///
/// Every path after the 2xx must acknowledge it. Returning without one leaves the far end
/// retransmitting for 32 seconds and then tearing down a dialog this side already reported.
async fn confirm_target(
    endpoint: &Handle,
    calls: &Calls,
    invite: &Request,
    response: &Response,
    target: Target,
    responses: sipx_transport::Responses,
    relay: DescriptionRelay,
) -> Result<OffMediaLeg> {
    let dialog = Dialog::from_response(invite, response).ok_or(Error::NoDialog)?;
    let leg_target = in_dialog_target(&dialog, target);
    let inbox = calls.register(&dialog);
    let ack = build_ack(endpoint, &dialog, &leg_target)?;
    endpoint
        .send_directly(ack.clone(), leg_target.clone())
        .await?;
    tokio::spawn(reack_retransmitted_2xx(
        endpoint.clone(),
        responses,
        ack,
        leg_target.clone(),
    ));
    Ok(OffMediaLeg {
        dialog,
        target: leg_target,
        inbox,
        relay,
        acknowledging: None,
        deferred: VecDeque::new(),
        ended: false,
    })
}

/// Answer the source invitation with the target endpoint's own description.
///
/// The target already has a confirmed dialog by this point, so every failure here ends it before
/// returning: an acceptance this side could not send does not make that ownership disappear.
async fn accept_source(
    endpoint: &Handle,
    invitation: crate::dispatch::CouplingInvitation,
    mut relay: DescriptionRelay,
    answer: &[u8],
    two: &mut OffMediaLeg,
) -> Result<OffMediaLeg> {
    let tag = invitation.tag();
    // Everything fallible runs before the invitation is claimed, so a crossing CANCEL can still
    // end an invitation this acceptance turned out not to be able to answer.
    let prepared = description(answer)
        .and_then(|answer| relay.relay(answer).map_err(Error::Relay))
        .and_then(|answer| accept(endpoint, &invitation.incoming, &tag, &answer))
        .and_then(|accepted| {
            let dialog =
                Dialog::from_request(&invitation.incoming.request, &tag).ok_or(Error::NoDialog)?;
            let sequence = sequence_of(&invitation.incoming.request, &Method::Invite)
                .ok_or(Error::NoDialog)?;
            Ok((accepted, dialog, sequence))
        });
    let (accepted, dialog, sequence) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            end_leg(endpoint, two).await;
            invitation
                .refuse(endpoint, 488, "Not Acceptable Here")
                .await?;
            return Err(error);
        }
    };
    if let Err(error) = invitation.claim_with_tag(&tag) {
        end_leg(endpoint, two).await;
        return Err(error);
    }
    let (incoming, inbox) = invitation.into_parts();
    let target = in_dialog_target(&dialog, Target::new(incoming.source, incoming.transport));
    endpoint.respond(&incoming.key, accepted.clone()).await?;
    let now = Instant::now();
    Ok(OffMediaLeg {
        dialog,
        target,
        inbox,
        relay,
        acknowledging: Some(Acknowledging {
            key: incoming.key,
            response: accepted,
            sequence,
            interval: T1,
            next: now + T1,
            deadline: now + TIMER_H,
        }),
        deferred: VecDeque::new(),
        ended: false,
    })
}

/// Send the mapped offer on the far leg, answering a collision there while it is outstanding.
///
/// The far inbox keeps being read for exactly the reason the media-terminating driver reads it:
/// a crossed offer needs its 491 while the collision is still real, and a request that waited in
/// the queue would be relayed after the glare had disappeared.
async fn relay_to(
    endpoint: &Handle,
    state: &mut CouplingState,
    far_leg: Leg,
    far: &mut OffMediaLeg,
    method: &Method,
    offer: &str,
) -> Result<(Result<Response>, bool)> {
    let request = in_dialog_offer(endpoint, far, method, offer)?;
    let mut responses = endpoint.send(request, far.target.clone()).await?;
    let mut inbox_closed = false;
    let outgoing = async {
        match responses.final_response().await {
            Some(response) if response.status.is_success() => Ok(response),
            Some(response) => Err(Error::Rejected {
                status: response.status.code(),
                reason: String::from_utf8_lossy(&response.reason).into_owned(),
            }),
            None => Err(Error::NoResponse),
        }
    };
    tokio::pin!(outgoing);
    let result = loop {
        tokio::select! {
            biased;
            received = far.inbox.recv(), if !inbox_closed && far.deferred.len() < DEFERRED_CAPACITY => {
                let Some(request) = received else {
                    inbox_closed = true;
                    continue;
                };
                let Some(axis) = offer_axis(&request) else {
                    far.deferred.push_back(request);
                    continue;
                };
                match state.begin_offer(far_leg, axis) {
                    OfferAction::Refuse { status } => {
                        let reason = if status == 491 {
                            "Request Pending"
                        } else {
                            "Server Internal Error"
                        };
                        Call::refuse_with(endpoint, &request, status, reason).await?;
                    }
                    OfferAction::Relay { .. } => far.deferred.push_back(request),
                }
            }
            result = &mut outgoing => break result,
        }
    };
    // RFC 3261 §13.2.2.4: a 2xx to an INVITE is acknowledged by this UAC core, and only a 2xx —
    // the transaction layer acknowledges a failure response itself, and UPDATE has no ACK.
    if result.is_ok() && method == &Method::Invite {
        let ack = build_ack(endpoint, &far.dialog, &far.target)?;
        endpoint.send_directly(ack, far.target.clone()).await?;
    }
    Ok((result, inbox_closed))
}

fn offer_axis(incoming: &Incoming) -> Option<OfferAxis> {
    if !crate::update::carries_offer(&incoming.request) {
        return None;
    }
    match incoming.request.method {
        Method::Invite => Some(OfferAxis::Reinvite),
        Method::Update => Some(OfferAxis::Update),
        _ => None,
    }
}

/// A fresh per-dialog description identity.
///
/// The session id is random and the version starts at one; both belong to the dialog rather than
/// to the endpoint whose description passes through, which is what keeps one leg's revisions from
/// being read as the other's.
fn fresh_origin(address: IpAddr) -> Origin {
    let id = rand::RngCore::next_u64(&mut rand::rng()) >> 16;
    Origin::new(address, id, 1)
}

fn source_offer(incoming: &Incoming) -> Result<&str> {
    if incoming.request.body().is_empty() {
        return Err(Error::Sdp(
            "an off-media coupling relays descriptions and has none of its own to offer".to_owned(),
        ));
    }
    description(incoming.request.body())
}

fn description(body: &[u8]) -> Result<&str> {
    std::str::from_utf8(body).map_err(|_| Error::Sdp("the body is not UTF-8".to_owned()))
}

fn sequence_of(request: &Request, method: &Method) -> Option<u32> {
    request
        .headers
        .typed::<CSeq>()
        .and_then(std::result::Result::ok)
        .filter(|cseq| &cseq.method == method)
        .map(|cseq| cseq.sequence)
}

/// The target INVITE, carrying the source endpoint's own description.
fn offer_invite(
    endpoint: &Handle,
    target: &Target,
    to: &Uri,
    options: &OffMediaOptions,
    offer: &str,
) -> Result<Request> {
    let via = format!(
        "SIP/2.0/{} {};rport;branch={}",
        target.transport.as_str(),
        endpoint.sent_by_for(target.transport),
        sipx_transport::new_branch()
    );
    let builder = RequestBuilder::new(Method::Invite, to.clone())
        .header(HeaderName::Via, Bytes::from(via))?
        .header(
            HeaderName::To,
            Bytes::from(format!("<{}>", String::from_utf8_lossy(&to.to_bytes()))),
        )?
        .header(
            HeaderName::From,
            Bytes::from(format!("{};tag={}", options.from, crate::call::token())),
        )?
        .header(
            HeaderName::CallId,
            Bytes::from(format!("{}@sipx", crate::call::token())),
        )?
        .cseq(1, &Method::Invite)?
        .header(
            HeaderName::Contact,
            Bytes::from(contact_for(endpoint, target.transport)),
        )?
        .max_forwards(70)
        // No `100rel`: a reliable provisional may carry an offer, and answering one needs a
        // description this role does not author. RFC 3262 §3 forbids the far end sending one
        // unless this request says it is supported.
        .header(
            HeaderName::Allow,
            Bytes::from_static(b"INVITE, ACK, BYE, CANCEL, UPDATE"),
        )?
        .header(
            HeaderName::ContentType,
            Bytes::from_static(b"application/sdp"),
        )?
        .body(Bytes::from(offer.to_owned()));
    Ok(builder.build())
}

/// An in-dialog offer on the far leg, on the axis it arrived on.
fn in_dialog_offer(
    endpoint: &Handle,
    leg: &mut OffMediaLeg,
    method: &Method,
    offer: &str,
) -> Result<Request> {
    let cseq = leg.dialog.next_cseq();
    let (local, remote) = leg.dialog.local_and_remote();
    let (uri, routes) = leg.dialog.request_target();
    let via = format!(
        "SIP/2.0/{} {};rport;branch={}",
        leg.target.transport.as_str(),
        endpoint.sent_by_for(leg.target.transport),
        sipx_transport::new_branch()
    );
    let builder = RequestBuilder::new(method.clone(), uri)
        .header(HeaderName::Via, Bytes::from(via))?
        .header(HeaderName::To, Bytes::from(remote))?
        .header(HeaderName::From, Bytes::from(local))?
        .header(
            HeaderName::CallId,
            Bytes::from(leg.dialog.id.call_id.clone()),
        )?
        .cseq(cseq, method)?
        .header(
            HeaderName::Contact,
            Bytes::from(contact_for(endpoint, leg.target.transport)),
        )?
        .header(
            HeaderName::ContentType,
            Bytes::from_static(b"application/sdp"),
        )?
        .max_forwards(70);
    Ok(add_routes(builder, &routes)?
        .body(Bytes::from(offer.to_owned()))
        .build())
}

/// The initial 2xx, with the relayed answer and this side's dialog tag.
fn accept(endpoint: &Handle, incoming: &Incoming, tag: &str, answer: &str) -> Result<Response> {
    let Some(to) = incoming.request.headers.value(&HeaderName::To) else {
        return Err(Error::NoDialog);
    };
    let to = format!("{};tag={tag}", String::from_utf8_lossy(&to));
    let status = StatusCode::new(200).ok_or(Error::NoDialog)?;
    let target = Target::new(incoming.source, incoming.transport);
    Ok(
        ResponseBuilder::to_request(&incoming.request, status, "OK")?
            .set_header(&HeaderName::To, Bytes::from(to))?
            .header(
                HeaderName::Contact,
                Bytes::from(contact_for(endpoint, target.transport)),
            )?
            .header(
                HeaderName::ContentType,
                Bytes::from_static(b"application/sdp"),
            )?
            .body(Bytes::from(answer.to_owned()))
            .build(),
    )
}

/// The 2xx to an in-dialog offer, carrying the peer endpoint's own description.
fn accept_in_dialog(
    endpoint: &Handle,
    incoming: &Incoming,
    target: &Target,
    answer: &str,
) -> Result<Response> {
    let status = StatusCode::new(200).ok_or(Error::NoDialog)?;
    Ok(
        ResponseBuilder::to_request(&incoming.request, status, "OK")?
            .header(
                HeaderName::Contact,
                Bytes::from(contact_for(endpoint, target.transport)),
            )?
            .header(
                HeaderName::ContentType,
                Bytes::from_static(b"application/sdp"),
            )?
            .body(Bytes::from(answer.to_owned()))
            .build(),
    )
}

async fn respond(
    endpoint: &Handle,
    incoming: &Incoming,
    status: u16,
    reason: &'static str,
    body: Option<Bytes>,
) -> Result<()> {
    let code = StatusCode::new(status).ok_or(Error::NoDialog)?;
    let mut builder = ResponseBuilder::to_request(&incoming.request, code, reason)?;
    if let Some(body) = body {
        builder = builder
            .header(
                HeaderName::ContentType,
                Bytes::from_static(b"application/sdp"),
            )?
            .body(body);
    }
    endpoint.respond(&incoming.key, builder.build()).await?;
    Ok(())
}

/// End one dialog with a BYE, if it has not ended already.
///
/// Failures are logged rather than returned: this runs on cleanup paths whose primary cause is
/// already on its way to the caller, and the transport counts the unsent request.
async fn end_leg(endpoint: &Handle, leg: &mut OffMediaLeg) {
    if leg.ended {
        return;
    }
    leg.ended = true;
    let cseq = leg.dialog.next_cseq();
    let (local, remote) = leg.dialog.local_and_remote();
    let (uri, routes) = leg.dialog.request_target();
    let built = RequestBuilder::new(Method::Bye, uri)
        .header(
            HeaderName::Via,
            Bytes::from(format!(
                "SIP/2.0/{} {};rport;branch={}",
                leg.target.transport.as_str(),
                endpoint.sent_by_for(leg.target.transport),
                sipx_transport::new_branch()
            )),
        )
        .and_then(|builder| builder.header(HeaderName::To, Bytes::from(remote)))
        .and_then(|builder| builder.header(HeaderName::From, Bytes::from(local)))
        .and_then(|builder| {
            builder.header(
                HeaderName::CallId,
                Bytes::from(leg.dialog.id.call_id.clone()),
            )
        })
        .and_then(|builder| builder.cseq(cseq, &Method::Bye))
        .map(|builder| builder.max_forwards(70))
        .and_then(|builder| add_routes(builder, &routes));
    let Ok(builder) = built else {
        tracing::warn!("could not build the BYE ending an off-media coupled dialog");
        return;
    };
    match endpoint.send(builder.build(), leg.target.clone()).await {
        Ok(mut responses) => {
            let _final = responses.final_response().await;
        }
        Err(error) => {
            tracing::warn!(%error, "could not end an off-media coupled dialog");
        }
    }
}

/// The target's final response, or the reason there will not be one.
async fn await_final(
    responses: &mut sipx_transport::Responses,
    timeout: Option<Duration>,
) -> Result<Response> {
    let final_response = async { responses.final_response().await.ok_or(Error::NoResponse) };
    match timeout {
        Some(limit) => tokio::time::timeout(limit, final_response)
            .await
            .unwrap_or(Err(Error::NoResponse)),
        None => final_response.await,
    }
}
