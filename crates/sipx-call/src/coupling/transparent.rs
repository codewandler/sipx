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
//! The early carriers are relayed the same way (`C-8`). `100rel` is mirrored onto the target
//! INVITE, so a reliable provisional carrying a description crosses into a reliable provisional
//! of this coupling's own, and PRACK is correlated on both legs — sent on the target one,
//! answered on the source one. An offerless INVITE is relayed as the offerless INVITE it is, and
//! RFC 3262 §5's delayed offer comes back from the target endpoint with its answer supplied by
//! the source endpoint's PRACK. In every case both descriptions belong to the endpoints; this
//! role still authors none, which is the whole of what it exists not to do.
//!
//! An offerless re-INVITE is still refused rather than half-done. An offerless initial INVITE
//! without `100rel`, however, uses the final carrier: the target offer is mapped into the source
//! `2xx`, and the target ACK is held until the source ACK supplies the mapped answer (`C-10`).

use std::collections::VecDeque;
use std::net::IpAddr;
use std::time::Duration;

use bytes::Bytes;
use sipx_sdp::relay::DescriptionRelay;
use sipx_sdp::session::Origin;
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::headers::CSeq;
use sipx_sip::rel::{Numbering, Offered, RAck, Received, Sequence};
use sipx_sip::transaction::TuEvent;
use sipx_sip::{HeaderName, Method, Request, Response, StatusCode, Uri};
use sipx_transport::{Handle, Incoming, Target};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::{
    CouplingEnd, CouplingState, DEFERRED_CAPACITY, FailureAction, Leg, OfferAction, OfferAxis,
};
use crate::call::{
    add_routes, build_ack, build_ack_with_body, contact_for, in_dialog_target,
    normal_clearing_reason, reack_retransmitted_2xx, sleep_until, withdraw,
};
use crate::dialog::Dialog;
use crate::dispatch::CouplingInvitation;
use crate::{Call, Calls, Error, Invitation, Result};

/// RFC 3261 §17.2.1 timers for retransmitting an INVITE's 2xx until its ACK.
const T1: Duration = Duration::from_millis(500);
const T2: Duration = Duration::from_secs(4);
const TIMER_H: Duration = Duration::from_secs(32);

/// What this role does inside a dialog it couples (RFC 3261 §20.5).
///
/// PRACK is on the list because the reliable-provisional carrier is relayed; REFER, INFO and
/// NOTIFY are not, because nothing here would know what to do with them. An advertisement wider
/// than the behaviour is worse than a narrow one — a peer that reads it and is then refused has
/// been told two different things by the same element.
const ALLOW: &[u8] = b"INVITE, ACK, BYE, CANCEL, UPDATE, PRACK";

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
    /// How long cleanup may wait for protocol completion events.
    ///
    /// This bounds both withdrawing the target invitation and, for a final-response delayed
    /// offer, holding the target ACK for the source ACK answer.
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

/// The carriers that run before either dialog of this coupling is confirmed (`C-8`).
///
/// RFC 3262's reliable provisional and PRACK, and RFC 3264's delayed offer. None of them changes
/// what the role does with a description — the endpoint's own bytes with one line replaced — so
/// what this holds is only the bookkeeping the two carriers need: the numbering of the
/// provisionals this side *sends* on the source leg (§3), the ordering of the ones it *receives*
/// on the target leg (§4), and the target PRACK that may not leave until the source has supplied
/// the answer §5 says it owes.
#[derive(Debug)]
struct EarlyCarriers {
    /// The source INVITE's `CSeq`, which every `RAck` arriving on that leg must name (§7.2).
    source_invite_cseq: u32,
    /// The target INVITE's `CSeq`, which every `RAck` this side sends must name.
    target_invite_cseq: u32,
    /// Whether the source INVITE carried an offer.
    ///
    /// It decides which half of RFC 3264 a target provisional's description is, and therefore
    /// whether the PRACK acknowledging it owes a description back (§5).
    source_offered: bool,
    /// The numbering of the reliable provisionals this side sends on the source leg (§3).
    numbering: Numbering,
    /// The ordering of the ones it receives on the target leg (§4).
    seen: Sequence,
    /// Stops the retransmission of the provisional last sent on the source leg.
    retransmission: Option<CancellationToken>,
    /// The target early dialog a reliable provisional established (RFC 3261 §12.1.1).
    dialog: Option<Dialog>,
    /// The `RSeq` of a target provisional whose PRACK is held for the source's answer (§5).
    held: Option<u32>,
    /// Whether the initial offer/answer exchange settled before either dialog was confirmed.
    settled: bool,
    /// The highest `CSeq` seen from the source before its dialog existed.
    remote_cseq: Option<u32>,
    /// Source-leg requests that arrived while the target INVITE was still outstanding.
    deferred: VecDeque<Incoming>,
}

impl EarlyCarriers {
    fn new(source: &Incoming, target: &Request, source_offered: bool) -> Self {
        Self {
            source_invite_cseq: sequence_of(&source.request, &Method::Invite).unwrap_or(1),
            target_invite_cseq: sequence_of(target, &Method::Invite).unwrap_or(1),
            source_offered,
            // §3: the first number "MUST be between 1 and 2**31 - 1", chosen uniformly. It is a
            // per-transaction secret as much as a counter: a predictable one lets an off-path
            // attacker forge the PRACK that stops the retransmissions.
            numbering: Numbering::starting_at({
                use rand::Rng as _;
                rand::rng().random_range(1..=sipx_sip::rel::MAX_FIRST_RSEQ)
            }),
            seen: Sequence::default(),
            retransmission: None,
            dialog: None,
            held: None,
            settled: false,
            remote_cseq: None,
            deferred: VecDeque::new(),
        }
    }

    /// Stop retransmitting the provisional this side last sent on the source leg.
    fn acknowledged(&mut self) {
        if let Some(stop) = self.retransmission.take() {
            stop.cancel();
        }
    }

    /// The sequence number the target leg's early dialog reached, if one exists.
    fn target_cseq(&self) -> Option<u32> {
        self.dialog.as_ref().map(|dialog| dialog.local_cseq)
    }

    /// The source-leg requests this phase set aside, handed to the confirmed leg.
    fn take_deferred(&mut self) -> VecDeque<Incoming> {
        std::mem::take(&mut self.deferred)
    }
}

impl Drop for EarlyCarriers {
    fn drop(&mut self) {
        // A retransmission task outlives this value otherwise, and would go on resending a
        // provisional for an invitation that has since been answered or abandoned.
        self.acknowledged();
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
    ///
    /// An offerless source INVITE is relayed as one. RFC 3262 §5 carries the delayed exchange on
    /// the reliable-provisional axis when `100rel` was offered; otherwise the target offer is
    /// mapped into the source `2xx` and its answer is mapped back in the held target ACK.
    #[allow(
        clippy::too_many_lines,
        reason = "the early carriers and the confirmation they lead to are one sequence"
    )]
    pub async fn dial(
        invitation: Invitation,
        calls: &Calls,
        endpoint: &Handle,
        target: Target,
        to: &Uri,
        options: &OffMediaOptions,
    ) -> Result<Self> {
        let reliability = Offered::in_request(&invitation.request().request);
        let mut two_relay = DescriptionRelay::new(fresh_origin(options.origin_address));
        let relayed = match source_offer(invitation.request()).and_then(|offer| {
            offer
                .map(|offer| two_relay.relay(offer).map_err(Error::Relay))
                .transpose()
        }) {
            Ok(relayed) => relayed,
            Err(error) => {
                invitation
                    .refuse(endpoint, 488, "Not Acceptable Here")
                    .await?;
                return Err(error);
            }
        };

        let mut invitation = invitation.into_coupling();
        let mut state = CouplingState::new();
        if relayed.is_some() {
            let _relay = state.begin_offer(Leg::One, OfferAxis::InitialInvite);
        }
        let cancellation = invitation.cancellation();
        let tag = invitation.tag();

        let invite = offer_invite(
            endpoint,
            &target,
            to,
            options,
            relayed.as_deref(),
            reliability,
        )?;
        let mut responses = endpoint.send(invite.clone(), target.clone()).await?;

        let mut one_relay = DescriptionRelay::new(fresh_origin(options.origin_address));
        let mut early = EarlyCarriers::new(&invitation.incoming, &invite, relayed.is_some());
        let response = tokio::select! {
            response = await_confirmation(
                endpoint,
                &mut invitation,
                &mut responses,
                &invite,
                &target,
                (&mut one_relay, &mut two_relay),
                &mut state,
                &mut early,
                &tag,
                options.timeout,
            ) => response,
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
                // The target INVITE may still be outstanding — an early carrier that could not
                // be relayed says nothing about whether the far end is still ringing — so it is
                // withdrawn before the source leg is given its final response.
                let _cleanup = withdraw(
                    endpoint,
                    &invite,
                    target.clone(),
                    &mut responses,
                    &normal_clearing_reason(),
                    options.cancellation_timeout,
                )
                .await;
                let (status, reason) = match error {
                    Error::Relay(_) | Error::Sdp(_) => (488, "Not Acceptable Here"),
                    _ => (503, "Service Unavailable"),
                };
                invitation.refuse(endpoint, status, reason).await?;
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

        let hold_target_ack = !early.source_offered && !early.settled;
        let (mut two, held_responses) = match confirm_target(
            endpoint,
            calls,
            &invite,
            &response,
            target,
            responses,
            two_relay,
            early.target_cseq(),
            hold_target_ack,
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

        let one = match held_responses {
            Some(responses) => {
                accept_delayed_source(
                    endpoint,
                    invitation,
                    one_relay,
                    response.body(),
                    &mut early,
                    &mut two,
                    responses,
                    options.cancellation_timeout,
                )
                .await?
            }
            None => {
                accept_source(
                    endpoint,
                    invitation,
                    one_relay,
                    response.body(),
                    &mut early,
                    &mut two,
                )
                .await?
            }
        };
        // A no-op when a reliable provisional already settled the exchange, which is the point:
        // the early carriers use the same policy object rather than a second one.
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

/// Drive the target INVITE to its final response, relaying the early carriers on the way.
///
/// Both inboxes are read here for the same reason the confirmed driver reads both: a reliable
/// provisional arrives on the target leg and the PRACK answering the one this side relayed
/// arrives on the source leg, and neither can wait for the other to finish. Source-leg requests
/// this phase has no answer for are set aside rather than dropped, and the confirmed leg
/// inherits them.
#[allow(
    clippy::too_many_arguments,
    reason = "the early phase borrows exactly the coupling state it is allowed to touch"
)]
async fn await_confirmation(
    endpoint: &Handle,
    invitation: &mut CouplingInvitation,
    responses: &mut sipx_transport::Responses,
    invite: &Request,
    target: &Target,
    relays: (&mut DescriptionRelay, &mut DescriptionRelay),
    state: &mut CouplingState,
    early: &mut EarlyCarriers,
    tag: &str,
    timeout: Option<Duration>,
) -> Result<Response> {
    let (one_relay, two_relay) = relays;
    let confirming = async {
        let mut source_closed = false;
        loop {
            tokio::select! {
                event = responses.next() => {
                    let Some(event) = event else { return Err(Error::NoResponse) };
                    let TuEvent::Response(response) = event else { continue };
                    if response.status.is_final() {
                        return Ok(*response);
                    }
                    relay_provisional(
                        endpoint, invitation, &response, invite, target, state, one_relay, early,
                        tag,
                    )
                    .await?;
                }
                received = invitation.requests.recv(),
                    if !source_closed && early.deferred.len() < DEFERRED_CAPACITY =>
                {
                    let Some(request) = received else {
                        source_closed = true;
                        continue;
                    };
                    if request.request.method == Method::Prack {
                        relay_prack(endpoint, &request, target, state, two_relay, early).await?;
                    } else {
                        // Nothing this phase can answer: an UPDATE belongs to the confirmed
                        // driver, which is where `accept_source` hands these on to.
                        early.deferred.push_back(request);
                    }
                }
            }
        }
    };
    match timeout {
        Some(limit) => tokio::time::timeout(limit, confirming)
            .await
            .unwrap_or(Err(Error::NoResponse)),
        None => confirming.await,
    }
}

/// Relay a reliable provisional from the target leg onto the source leg (RFC 3262).
///
/// The description it carries is the target endpoint's own, and it reaches the source endpoint
/// with only its `o=` line replaced — the same mapping the confirmed carriers use. Which half of
/// RFC 3264 it is depends on whether the source INVITE offered, and that decides the one thing
/// this carrier adds: whether the PRACK acknowledging it owes a description back (§5).
#[allow(
    clippy::too_many_arguments,
    reason = "relaying one provisional touches both legs' relays, numbering and policy"
)]
async fn relay_provisional(
    endpoint: &Handle,
    invitation: &CouplingInvitation,
    response: &Response,
    invite: &Request,
    target: &Target,
    state: &mut CouplingState,
    one_relay: &mut DescriptionRelay,
    early: &mut EarlyCarriers,
    tag: &str,
) -> Result<()> {
    // §5 permits a description only in a *reliable* provisional, so an unreliable one carries
    // nothing this role could relay and reaches the source leg as nothing at all.
    let Some(rseq) = crate::rel::reliable_sequence(response) else {
        return Ok(());
    };
    // §4: a retransmission "MUST be discarded", and one arriving out of order "MUST NOT be
    // acknowledged with a PRACK, and MUST NOT be processed further".
    if early.seen.accept(rseq) != Received::Acknowledge {
        return Ok(());
    }
    if early.dialog.is_none() {
        // §4: "the provisional response MUST establish a dialog if one is not yet created", and
        // the PRACK has to go inside it — a UAS with no matching transaction is what a PRACK sent
        // outside one reaches.
        early.dialog = Some(Dialog::from_response(invite, response).ok_or(Error::NoDialog)?);
    }
    // §3: this side may not number a second reliable provisional while the first is
    // unacknowledged. Leaving this one unrelayed also leaves it un-PRACKed, so the target
    // retransmits and eventually fails its own invitation — an honest end to a source leg that
    // has stopped acknowledging.
    let Some(number) = early.numbering.allocate() else {
        return Ok(());
    };
    // Mapped before the source leg is told anything. A description this side cannot put in front
    // of the other endpoint stops here: the caller withdraws the target invitation and refuses
    // the source one, rather than relaying a description it would have had to invent.
    let mapped = match description_or_none(response.body()) {
        Some(carried) => Some(one_relay.relay(carried?).map_err(Error::Relay)?),
        None => None,
    };

    let provisional = source_provisional(
        endpoint,
        &invitation.incoming,
        tag,
        response,
        number,
        mapped.as_deref(),
    )?;
    endpoint
        .respond(&invitation.incoming.key, provisional.clone())
        .await?;
    early.acknowledged();
    let stop = CancellationToken::new();
    tokio::spawn(crate::rel::retransmit_until_pracked(
        endpoint.clone(),
        invitation.incoming.key.clone(),
        provisional,
        stop.clone(),
    ));
    early.retransmission = Some(stop);

    if mapped.is_some() && !early.source_offered {
        // §5: the INVITE offered nothing, so this description is the delayed offer and its
        // answer travels in PRACK. The target PRACK is held until the source supplies one.
        match state.begin_offer(Leg::Two, OfferAxis::ReliableProvisional) {
            OfferAction::Relay { .. } => {}
            OfferAction::Refuse { .. } => return Err(Error::NoDialog),
        }
        early.held = Some(rseq);
        return Ok(());
    }
    if mapped.is_some() {
        // The INVITE offered, so this is the answer to it and the exchange settles here.
        let _settled = state.complete(Leg::One);
        early.settled = true;
    }
    let invite_cseq = early.target_invite_cseq;
    let dialog = early.dialog.as_mut().ok_or(Error::NoDialog)?;
    prack_target(endpoint, dialog, target, rseq, invite_cseq, None).await
}

/// Answer a PRACK on the source leg, and release the target one it may have been holding.
///
/// The correlation is RFC 3262 §3's: same dialog, and an `RAck` naming the number this side
/// allocated together with the source INVITE's own `CSeq` and method. One that matches nothing
/// gets `481` rather than silence, which is what tells a peer the two sides disagree instead of
/// leaving its transaction to look like a lost packet.
async fn relay_prack(
    endpoint: &Handle,
    incoming: &Incoming,
    target: &Target,
    state: &mut CouplingState,
    two_relay: &mut DescriptionRelay,
    early: &mut EarlyCarriers,
) -> Result<()> {
    let acknowledged = incoming
        .request
        .headers
        .typed::<RAck>()
        .and_then(std::result::Result::ok)
        .is_some_and(|ack| {
            early
                .numbering
                .acknowledge(&ack, early.source_invite_cseq, Method::Invite.as_bytes())
        });
    if !acknowledged {
        respond(
            endpoint,
            incoming,
            481,
            "Call/Transaction Does Not Exist",
            None,
        )
        .await?;
        return Ok(());
    }
    early.remote_cseq = early
        .remote_cseq
        .max(sequence_of(&incoming.request, &Method::Prack));
    early.acknowledged();
    let Some(rseq) = early.held.take() else {
        respond(endpoint, incoming, 200, "OK", None).await?;
        return Ok(());
    };

    // §5: the source INVITE offered nothing, so the answer to the relayed offer is in this PRACK
    // and nowhere else. Mapped on the leg it arrived on, before the target leg is told anything:
    // one this side cannot carry is refused here and the target never sees a PRACK at all.
    let mapped = description_or_none(incoming.request.body())
        .unwrap_or_else(|| {
            Err(Error::Sdp(
                "the PRACK answering a delayed offer carried no description".to_owned(),
            ))
        })
        .and_then(|answer| two_relay.relay(answer).map_err(Error::Relay));
    let answer = match mapped {
        Ok(answer) => answer,
        Err(error) => {
            let _settled = state.fail(Leg::Two);
            respond(endpoint, incoming, 488, "Not Acceptable Here", None).await?;
            return Err(error);
        }
    };
    let invite_cseq = early.target_invite_cseq;
    let dialog = early.dialog.as_mut().ok_or(Error::NoDialog)?;
    let relayed = prack_target(endpoint, dialog, target, rseq, invite_cseq, Some(&answer)).await;
    match relayed {
        Ok(()) => {
            let _settled = state.complete(Leg::Two);
            early.settled = true;
            respond(endpoint, incoming, 200, "OK", None).await?;
            Ok(())
        }
        Err(error) => {
            let _settled = state.fail(Leg::Two);
            respond(endpoint, incoming, 488, "Not Acceptable Here", None).await?;
            Err(error)
        }
    }
}

/// Take ownership of the confirmed target dialog: register its inbox and acknowledge its 2xx.
///
/// Every path after the 2xx must acknowledge it. Returning without one leaves the far end
/// retransmitting for 32 seconds and then tearing down a dialog this side already reported.
#[allow(
    clippy::too_many_arguments,
    reason = "confirmation inherits the early dialog's numbering along with the 2xx"
)]
async fn confirm_target(
    endpoint: &Handle,
    calls: &Calls,
    invite: &Request,
    response: &Response,
    target: Target,
    responses: sipx_transport::Responses,
    relay: DescriptionRelay,
    early_cseq: Option<u32>,
    hold_ack: bool,
) -> Result<(OffMediaLeg, Option<sipx_transport::Responses>)> {
    let mut dialog = Dialog::from_response(invite, response).ok_or(Error::NoDialog)?;
    let leg_target = in_dialog_target(&dialog, target);
    let inbox = calls.register(&dialog);
    // Built before the early numbering is carried across: the ACK for a 2xx repeats the INVITE's
    // own sequence number rather than taking a new one (RFC 3261 §13.2.2.4).
    let ack = (!hold_ack)
        .then(|| build_ack(endpoint, &dialog, &leg_target))
        .transpose()?;
    // RFC 3261 §12.2.1.1: a PRACK sent while this dialog was early consumed a number in the same
    // sequence space, and a dialog rebuilt from the 2xx would hand that number out a second time
    // — putting the first in-dialog request behind one the peer has already seen.
    if let Some(early_cseq) = early_cseq {
        dialog.local_cseq = dialog.local_cseq.max(early_cseq);
    }
    let held_responses = match ack {
        Some(ack) => {
            endpoint
                .send_directly(ack.clone(), leg_target.clone())
                .await?;
            tokio::spawn(reack_retransmitted_2xx(
                endpoint.clone(),
                responses,
                ack,
                leg_target.clone(),
            ));
            None
        }
        None => Some(responses),
    };
    let leg = OffMediaLeg {
        dialog,
        target: leg_target,
        inbox,
        relay,
        acknowledging: None,
        deferred: VecDeque::new(),
        ended: false,
    };
    Ok((leg, held_responses))
}

/// Answer the source invitation with the target endpoint's own description.
///
/// The target already has a confirmed dialog by this point, so every failure here ends it before
/// returning: an acceptance this side could not send does not make that ownership disappear.
///
/// The `2xx` carries a description only when one is still owed. An exchange RFC 3262 §5 already
/// settled in a reliable provisional is complete, and repeating it here would be a renegotiation
/// neither endpoint asked for; a `2xx` with no description and no settled exchange is a target
/// that never answered at all, which is a failure rather than an acceptance.
async fn accept_source(
    endpoint: &Handle,
    invitation: CouplingInvitation,
    mut relay: DescriptionRelay,
    answer: &[u8],
    early: &mut EarlyCarriers,
    two: &mut OffMediaLeg,
) -> Result<OffMediaLeg> {
    let tag = invitation.tag();
    let settled = early.settled;
    // Everything fallible runs before the invitation is claimed, so a crossing CANCEL can still
    // end an invitation this acceptance turned out not to be able to answer.
    let prepared = description_or_none(answer)
        .map_or_else(
            || {
                settled.then_some(None).ok_or_else(|| {
                    Error::Sdp(
                        "the target accepted without ever answering the relayed offer".to_owned(),
                    )
                })
            },
            |answer| {
                answer
                    .and_then(|answer| relay.relay(answer).map_err(Error::Relay))
                    .map(Some)
            },
        )
        .and_then(|answer| accept(endpoint, &invitation.incoming, &tag, answer.as_deref()))
        .and_then(|accepted| {
            let mut dialog =
                Dialog::from_request(&invitation.incoming.request, &tag).ok_or(Error::NoDialog)?;
            // A PRACK answered while the dialog was early already advanced the peer's sequence
            // space, and forgetting that would accept a replay of it as a fresh request.
            dialog.remote_cseq = dialog.remote_cseq.max(early.remote_cseq);
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
        // Requests the early phase had no answer for. Set aside rather than dropped: the
        // confirmed driver is where an UPDATE that raced the target's 2xx belongs.
        deferred: early.take_deferred(),
        ended: false,
    })
}

/// Carry the final-response delayed offer across both dialogs.
///
/// The target's 2xx has established a dialog but its ACK is deliberately retained until the
/// source ACK supplies the answer. No media session exists here: both descriptions pass through
/// the per-leg relays before either far endpoint is told the exchange completed.
#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the held ACK joins the two invitations, relays, inboxes and one explicit bound"
)]
async fn accept_delayed_source(
    endpoint: &Handle,
    invitation: CouplingInvitation,
    mut relay: DescriptionRelay,
    offer: &[u8],
    early: &mut EarlyCarriers,
    two: &mut OffMediaLeg,
    responses: sipx_transport::Responses,
    held_ack_timeout: Duration,
) -> Result<OffMediaLeg> {
    let tag = invitation.tag();
    let prepared = description_or_none(offer)
        .unwrap_or_else(|| {
            Err(Error::Sdp(
                "the target accepted an offerless INVITE without supplying an offer".to_owned(),
            ))
        })
        .and_then(|offer| relay.relay(offer).map_err(Error::Relay))
        .and_then(|offer| accept(endpoint, &invitation.incoming, &tag, Some(&offer)))
        .and_then(|accepted| {
            let mut dialog =
                Dialog::from_request(&invitation.incoming.request, &tag).ok_or(Error::NoDialog)?;
            dialog.remote_cseq = dialog.remote_cseq.max(early.remote_cseq);
            let sequence = sequence_of(&invitation.incoming.request, &Method::Invite)
                .ok_or(Error::NoDialog)?;
            Ok((accepted, dialog, sequence))
        });
    let (accepted, dialog, sequence) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            abandon_delayed_exchange(endpoint, None, two, responses).await;
            invitation
                .refuse(endpoint, 488, "Not Acceptable Here")
                .await?;
            return Err(error);
        }
    };
    if let Err(error) = invitation.claim_with_tag(&tag) {
        abandon_delayed_exchange(endpoint, None, two, responses).await;
        return Err(error);
    }

    let (incoming, inbox) = invitation.into_parts();
    let target = in_dialog_target(&dialog, Target::new(incoming.source, incoming.transport));
    let now = Instant::now();
    let mut one = OffMediaLeg {
        dialog,
        target,
        inbox,
        relay,
        acknowledging: Some(Acknowledging {
            key: incoming.key.clone(),
            response: accepted.clone(),
            sequence,
            interval: T1,
            next: now + T1,
            deadline: now + TIMER_H,
        }),
        deferred: early.take_deferred(),
        ended: false,
    };
    if let Err(error) = endpoint.respond(&incoming.key, accepted).await {
        abandon_delayed_exchange(endpoint, Some(&mut one), two, responses).await;
        return Err(error.into());
    }

    let deadline = now + held_ack_timeout;
    let answer = if held_ack_timeout.is_zero() {
        Err(Error::NoResponse)
    } else {
        loop {
            let retransmit = one.retransmit_at();
            tokio::select! {
                biased;
                request = one.inbox.recv() => {
                    let Some(request) = request else {
                        break Err(Error::NoResponse);
                    };
                    if request.request.method == Method::Ack
                        && one.dialog.matches(&request.request)
                        && one.acknowledged_by(&request.request)
                    {
                        let mapped = description_or_none(request.request.body())
                            .unwrap_or_else(|| {
                                Err(Error::Sdp(
                                    "the ACK answering the delayed offer carried no description"
                                        .to_owned(),
                                ))
                            })
                            .and_then(|answer| two.relay.relay(answer).map_err(Error::Relay));
                        break mapped;
                    }
                    if request.request.method != Method::Ack
                        && one.deferred.len() < DEFERRED_CAPACITY
                    {
                        one.deferred.push_back(request);
                    }
                }
                () = sleep_until(retransmit), if retransmit.is_some() => {
                    one.retransmit(endpoint).await?;
                }
                () = tokio::time::sleep_until(deadline) => break Err(Error::NoResponse),
            }
        }
    };
    let answer = match answer {
        Ok(answer) => answer,
        Err(error) => {
            abandon_delayed_exchange(endpoint, Some(&mut one), two, responses).await;
            return Err(error);
        }
    };

    if let Err(error) = release_held_target_ack(endpoint, two, responses, Some(&answer)).await {
        // The mapped ACK could not leave. A bodiless retry is still preferable to abandoning the
        // target to its complete 2xx lifetime; either way both dialogs are ended best-effort.
        if let Ok(ack) = build_ack(endpoint, &two.dialog, &two.target) {
            // discard: this fallback ACK is best effort on an already-failing path; the endpoint
            // counts a send failure, while the mapped ACK error remains the returned cause.
            let _ = endpoint.send_directly(ack, two.target.clone()).await;
        }
        end_leg_without_wait(endpoint, two).await;
        end_leg_without_wait(endpoint, &mut one).await;
        return Err(error);
    }
    Ok(one)
}

/// Release a target 2xx with the mapped delayed answer, retaining the response stream so every
/// retransmitted 2xx is acknowledged with the same bytes.
async fn release_held_target_ack(
    endpoint: &Handle,
    two: &OffMediaLeg,
    responses: sipx_transport::Responses,
    answer: Option<&str>,
) -> Result<()> {
    let ack = build_ack_with_body(endpoint, &two.dialog, &two.target, answer)?;
    endpoint
        .send_directly(ack.clone(), two.target.clone())
        .await?;
    tokio::spawn(reack_retransmitted_2xx(
        endpoint.clone(),
        responses,
        ack,
        two.target.clone(),
    ));
    Ok(())
}

/// Stop a failed delayed exchange without leaving the target retransmitting its 2xx for Timer H.
async fn abandon_delayed_exchange(
    endpoint: &Handle,
    one: Option<&mut OffMediaLeg>,
    two: &mut OffMediaLeg,
    responses: sipx_transport::Responses,
) {
    // The bodiless ACK deliberately cannot be mistaken for a valid answer. Its only job is to
    // stop the target's 2xx lifetime before both confirmed dialogs are ended with BYE.
    let _released = release_held_target_ack(endpoint, two, responses, None).await;
    end_leg_without_wait(endpoint, two).await;
    if let Some(one) = one {
        end_leg_without_wait(endpoint, one).await;
    }
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

/// The source INVITE's own offer, or `None` when it is a delayed one this role may relay.
///
/// An offerless INVITE is a delayed offer. RFC 3262 §5 may carry it reliably before confirmation;
/// without that extension the final response and ACK carry the same exchange.
fn source_offer(incoming: &Incoming) -> Result<Option<&str>> {
    let Some(body) = description_or_none(incoming.request.body()) else {
        return Ok(None);
    };
    body.map(Some)
}

/// A body as text, or `None` when there is no body at all.
///
/// The two are different answers: an empty body is a message that negotiates nothing, and a body
/// that will not decode is one this role cannot carry.
fn description_or_none(body: &[u8]) -> Option<Result<&str>> {
    if body.is_empty() {
        return None;
    }
    Some(description(body))
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

/// The target INVITE, carrying the source endpoint's own description if it wrote one.
///
/// `100rel` is mirrored rather than asserted. RFC 3262 §3 lets the target put a description in a
/// reliable provisional only if this request says the extension is supported, and a description
/// arriving there can be relayed onward only if the source leg will accept one too. Claiming
/// support the source never offered would leave this role holding a description with nowhere to
/// put it — and putting it somewhere is the only thing it does.
fn offer_invite(
    endpoint: &Handle,
    target: &Target,
    to: &Uri,
    options: &OffMediaOptions,
    offer: Option<&str>,
    reliability: Offered,
) -> Result<Request> {
    let via = format!(
        "SIP/2.0/{} {};rport;branch={}",
        target.transport.as_str(),
        endpoint.sent_by_for(target.transport),
        sipx_transport::new_branch()
    );
    let mut builder = RequestBuilder::new(Method::Invite, to.clone())
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
        .header(HeaderName::Allow, Bytes::from_static(ALLOW))?;
    if reliability.supported || reliability.required {
        builder = builder.header(HeaderName::Supported, Bytes::from_static(b"100rel"))?;
    }
    if reliability.required {
        builder = builder.header(HeaderName::Require, Bytes::from_static(b"100rel"))?;
    }
    if let Some(offer) = offer {
        builder = builder
            .header(
                HeaderName::ContentType,
                Bytes::from_static(b"application/sdp"),
            )?
            .body(Bytes::from(offer.to_owned()));
    }
    Ok(builder.build())
}

/// PRACK a reliable provisional on the target leg (RFC 3262 §4).
///
/// `answer` is present in exactly one case: RFC 3262 §5's, where the INVITE offered nothing and
/// "the UAC ... MUST generate an answer in the PRACK". It is the source endpoint's own
/// description with one line replaced, never a re-serialization of it — an off-media element may
/// not normalize a description it is only carrying.
async fn prack_target(
    endpoint: &Handle,
    leg: &mut Dialog,
    target: &Target,
    rseq: u32,
    invite_cseq: u32,
    answer: Option<&str>,
) -> Result<()> {
    let cseq = leg.next_cseq();
    let (local, remote) = leg.local_and_remote();
    let (uri, routes) = leg.request_target();
    let ack = RAck {
        rseq,
        cseq: invite_cseq,
        method: Method::Invite.as_bytes().to_vec(),
    };
    let via = format!(
        "SIP/2.0/{} {};rport;branch={}",
        target.transport.as_str(),
        endpoint.sent_by_for(target.transport),
        sipx_transport::new_branch()
    );
    let mut builder = RequestBuilder::new(Method::Prack, uri)
        .header(HeaderName::Via, Bytes::from(via))?
        .header(HeaderName::To, Bytes::from(remote))?
        .header(HeaderName::From, Bytes::from(local))?
        .header(HeaderName::CallId, Bytes::from(leg.id.call_id.clone()))?
        .cseq(cseq, &Method::Prack)?
        .header(HeaderName::RAck, Bytes::from(ack.to_string()))?
        .max_forwards(70);
    if let Some(answer) = answer {
        builder = builder
            .header(
                HeaderName::ContentType,
                Bytes::from_static(b"application/sdp"),
            )?
            .body(Bytes::from(answer.to_owned()));
    }
    let request = add_routes(builder, &routes)?.build();
    let mut responses = endpoint.send(request, target.clone()).await?;
    // §3: a matching PRACK "MUST be responded to with a 2xx". A 481 means the target has no
    // record of the provisional this side just acknowledged, which is worth surfacing: the two
    // legs would otherwise go on disagreeing about which description is in force.
    match responses.final_response().await {
        Some(response) if response.status.is_success() => Ok(()),
        Some(response) => Err(Error::Rejected {
            status: response.status.code(),
            reason: String::from_utf8_lossy(&response.reason).into_owned(),
        }),
        None => Err(Error::NoResponse),
    }
}

/// The reliable provisional this coupling sends on the source leg (RFC 3262 §3).
///
/// The status and reason are the target's own, because this is a relay rather than a progress
/// report of its own making, and the description — when the target wrote one — reaches the
/// source endpoint with only its `o=` line replaced.
fn source_provisional(
    endpoint: &Handle,
    incoming: &Incoming,
    tag: &str,
    relayed: &Response,
    rseq: u32,
    body: Option<&str>,
) -> Result<Response> {
    let Some(to) = incoming.request.headers.value(&HeaderName::To) else {
        return Err(Error::NoDialog);
    };
    let to = format!("{};tag={tag}", String::from_utf8_lossy(&to));
    let target = Target::new(incoming.source, incoming.transport);
    let mut builder =
        ResponseBuilder::to_request(&incoming.request, relayed.status, relayed.reason.clone())?
            .set_header(&HeaderName::To, Bytes::from(to))?
            .header(
                HeaderName::Contact,
                Bytes::from(contact_for(endpoint, target.transport)),
            )?
            .header(HeaderName::Require, Bytes::from_static(b"100rel"))?
            .header(HeaderName::RSeq, Bytes::from(rseq.to_string()))?
            .header(HeaderName::Allow, Bytes::from_static(ALLOW))?;
    if let Some(body) = body {
        builder = builder
            .header(
                HeaderName::ContentType,
                Bytes::from_static(b"application/sdp"),
            )?
            .body(Bytes::from(body.to_owned()));
    }
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

/// The initial 2xx, with this side's dialog tag and the relayed answer when one is still owed.
fn accept(
    endpoint: &Handle,
    incoming: &Incoming,
    tag: &str,
    answer: Option<&str>,
) -> Result<Response> {
    let Some(to) = incoming.request.headers.value(&HeaderName::To) else {
        return Err(Error::NoDialog);
    };
    let to = format!("{};tag={tag}", String::from_utf8_lossy(&to));
    let status = StatusCode::new(200).ok_or(Error::NoDialog)?;
    let target = Target::new(incoming.source, incoming.transport);
    let mut builder = ResponseBuilder::to_request(&incoming.request, status, "OK")?
        .set_header(&HeaderName::To, Bytes::from(to))?
        .header(
            HeaderName::Contact,
            Bytes::from(contact_for(endpoint, target.transport)),
        )?
        .header(HeaderName::Allow, Bytes::from_static(ALLOW))?;
    if let Some(answer) = answer {
        builder = builder
            .header(
                HeaderName::ContentType,
                Bytes::from_static(b"application/sdp"),
            )?
            .body(Bytes::from(answer.to_owned()));
    }
    Ok(builder.build())
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
    let Some(request) = end_leg_request(endpoint, leg) else {
        return;
    };
    match endpoint.send(request, leg.target.clone()).await {
        Ok(mut responses) => {
            let _final = responses.final_response().await;
        }
        Err(error) => {
            tracing::warn!(%error, "could not end an off-media coupled dialog");
        }
    }
}

/// Start ending a failed setup without waiting on a peer whose ACK exchange has already failed.
async fn end_leg_without_wait(endpoint: &Handle, leg: &mut OffMediaLeg) {
    let Some(request) = end_leg_request(endpoint, leg) else {
        return;
    };
    if let Err(error) = endpoint.send(request, leg.target.clone()).await {
        tracing::warn!(%error, "could not end a failed off-media coupled dialog");
    }
}

fn end_leg_request(endpoint: &Handle, leg: &mut OffMediaLeg) -> Option<Request> {
    if leg.ended {
        return None;
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
        return None;
    };
    Some(builder.build())
}
