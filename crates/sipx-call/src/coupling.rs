//! Two dialogs driven as one call.
//!
//! [`CouplingState`] is the sans-I/O offer/answer and lifecycle policy. [`EarlyCoupling`] owns the
//! joined pending legs through cancellation, refusal or confirmation; [`Coupling`] then owns the
//! two calls, optional media bridge, and confirmed signalling loop. Listener configuration,
//! initial leg creation, routing and target choice stay above this crate.

use std::collections::VecDeque;
use std::net::IpAddr;

use sipx_media::Bridge;
use sipx_sdp::Direction;
use sipx_sip::{Method, Reason};
use sipx_transport::Incoming;
use tokio::sync::mpsc;

use crate::call::{CouplingDialEvent, sleep_until};
use crate::dispatch::CouplingInvitation;
use crate::{Call, Dialing, Error, Invitation, Result, Ringing};

const DEFERRED_CAPACITY: usize = 16;

/// One leg of a coupling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leg {
    /// The first leg supplied to the coupling.
    One,
    /// The second leg supplied to the coupling.
    Two,
}

impl Leg {
    const fn peer(self) -> Self {
        match self {
            Self::One => Self::Two,
            Self::Two => Self::One,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PerLeg<T> {
    one: T,
    two: T,
}

impl<T> PerLeg<T> {
    const fn new(one: T, two: T) -> Self {
        Self { one, two }
    }

    const fn get(&self, leg: Leg) -> &T {
        match leg {
            Leg::One => &self.one,
            Leg::Two => &self.two,
        }
    }

    const fn get_mut(&mut self, leg: Leg) -> &mut T {
        match leg {
            Leg::One => &mut self.one,
            Leg::Two => &mut self.two,
        }
    }
}

/// Where an SDP offer legally arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferAxis {
    /// The initial INVITE.
    InitialInvite,
    /// A reliable provisional response.
    ReliableProvisional,
    /// PRACK.
    Prack,
    /// UPDATE.
    Update,
    /// An in-dialog INVITE.
    Reinvite,
}

/// What the offer/answer policy asks its I/O driver to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferAction {
    /// Relay this offer to the peer leg.
    Relay {
        /// The leg on which the offer arrived.
        source: Leg,
        /// The carrier on which its answer must return.
        axis: OfferAxis,
    },
    /// Refuse the offer on its source leg.
    Refuse {
        /// The SIP status to send.
        status: u16,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NegotiationState {
    Idle,
    Offering(OfferAxis),
    Answering(OfferAxis),
}

impl NegotiationState {
    const fn is_idle(self) -> bool {
        matches!(self, Self::Idle)
    }
}

/// The action an inbound CANCEL has on the peer leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelAction {
    /// Cancel the peer's still-pending INVITE.
    CancelPeer,
    /// The peer confirmed while the source INVITE remained pending; end that dialog with BYE.
    ByePeer,
    /// The peer is already confirmed; CANCEL cannot erase its dialog.
    AcknowledgeOnly,
}

/// What a final failure on one leg requires on its peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureAction {
    /// Refuse the peer's still-pending INVITE with the same final status.
    RejectPeer {
        /// The status received on the failed outbound leg.
        status: u16,
    },
    /// The peer is confirmed, so end it with BYE rather than inventing a final INVITE response.
    ByePeer,
}

/// Sans-I/O policy shared by early and confirmed coupling drivers.
#[derive(Debug)]
pub struct CouplingState {
    negotiation: PerLeg<NegotiationState>,
    confirmed: PerLeg<bool>,
}

impl Default for CouplingState {
    fn default() -> Self {
        Self {
            negotiation: PerLeg::new(NegotiationState::Idle, NegotiationState::Idle),
            confirmed: PerLeg::new(false, false),
        }
    }
}

impl CouplingState {
    /// A fresh early coupling, with neither dialog confirmed.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that one leg has reached a confirmed dialog.
    pub fn confirm(&mut self, leg: Leg) {
        *self.confirmed.get_mut(leg) = true;
    }

    /// Whether this leg has reached a confirmed dialog.
    #[must_use]
    pub fn is_confirmed(&self, leg: Leg) -> bool {
        *self.confirmed.get(leg)
    }

    /// Begin relaying an offer.
    ///
    /// A collision with an offer this coupling sent on the source leg is refused 491. The remote
    /// UAC owns the randomized retry required by RFC 3261 §14.1; when it arrives after completion,
    /// it enters through this method as a new transaction.
    #[must_use]
    pub fn begin_offer(&mut self, source: Leg, axis: OfferAxis) -> OfferAction {
        let peer = source.peer();
        match *self.negotiation.get(source) {
            NegotiationState::Offering(_) => {
                return OfferAction::Refuse { status: 491 };
            }
            NegotiationState::Answering(_) => return OfferAction::Refuse { status: 500 },
            NegotiationState::Idle => {}
        }
        if !self.negotiation.get(peer).is_idle() {
            return OfferAction::Refuse { status: 491 };
        }
        self.start(source, axis)
    }

    /// Complete the exchange sourced on `source`.
    ///
    /// Returns whether an exchange from that leg was outstanding.
    #[must_use]
    pub fn complete(&mut self, source: Leg) -> bool {
        if !self.matches_exchange(source) {
            return false;
        }
        self.clear_exchange(source);
        true
    }

    /// Fail the exchange sourced on `source`.
    ///
    /// Failure settles the offer/answer axis just as a final answer does; there is no answer left
    /// outstanding after a final refusal.
    #[must_use]
    pub fn fail(&mut self, source: Leg) -> bool {
        self.complete(source)
    }

    /// Whether both per-leg offer/answer machines are idle.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.negotiation.one.is_idle() && self.negotiation.two.is_idle()
    }

    /// Map a CANCEL on `source` according to the peer leg's confirmation state.
    #[must_use]
    pub fn cancel(&self, source: Leg) -> CancelAction {
        match (self.is_confirmed(source), self.is_confirmed(source.peer())) {
            (false, false) => CancelAction::CancelPeer,
            (false, true) => CancelAction::ByePeer,
            (true, _) => CancelAction::AcknowledgeOnly,
        }
    }

    /// Map a final failure on `source` according to the peer leg's confirmation state.
    #[must_use]
    pub fn final_failure(&self, source: Leg, status: u16) -> FailureAction {
        if self.is_confirmed(source.peer()) {
            FailureAction::ByePeer
        } else {
            FailureAction::RejectPeer { status }
        }
    }

    fn start(&mut self, source: Leg, axis: OfferAxis) -> OfferAction {
        let peer = source.peer();
        *self.negotiation.get_mut(source) = NegotiationState::Answering(axis);
        *self.negotiation.get_mut(peer) = NegotiationState::Offering(axis);
        OfferAction::Relay { source, axis }
    }

    fn matches_exchange(&self, source: Leg) -> bool {
        let peer = source.peer();
        matches!(
            (*self.negotiation.get(source), *self.negotiation.get(peer)),
            (NegotiationState::Answering(one), NegotiationState::Offering(two)) if one == two
        )
    }

    fn clear_exchange(&mut self, source: Leg) {
        *self.negotiation.get_mut(source) = NegotiationState::Idle;
        *self.negotiation.get_mut(source.peer()) = NegotiationState::Idle;
    }
}

/// Why the confirmed-dialog driver returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CouplingEnd {
    /// One leg received and accepted a BYE.
    Bye(Leg),
    /// One routed inbox closed, so the peer was ended rather than orphaned.
    InboxClosed(Leg),
}

/// The two calls and routed inboxes produced by an early coupling once both dialogs confirm.
#[derive(Debug)]
pub struct ConfirmedCoupling {
    coupling: Coupling,
    one_incoming: mpsc::Receiver<Incoming>,
    two_incoming: mpsc::Receiver<Incoming>,
}

impl ConfirmedCoupling {
    /// Take the confirmed owner and its two routed request streams.
    #[must_use]
    pub fn into_parts(self) -> (Coupling, mpsc::Receiver<Incoming>, mpsc::Receiver<Incoming>) {
        (self.coupling, self.one_incoming, self.two_incoming)
    }
}

/// Two pending user-agent legs owned from an early dialog through confirmation or failure.
#[derive(Debug)]
pub struct EarlyCoupling {
    invitation: CouplingInvitation,
    ringing: Ringing,
    dialing: Option<Dialing>,
    outgoing_incoming: mpsc::Receiver<Incoming>,
    endpoint: sipx_transport::Handle,
    media_address: IpAddr,
    state: CouplingState,
    deferred: PerLeg<VecDeque<Incoming>>,
}

impl EarlyCoupling {
    /// Join an inbound invitation already rung to its pending outbound invitation.
    ///
    /// Routing and target selection happen before this handoff. From this point the coupling is
    /// the sole owner of the invitation, ringing state and dialing state, including their media
    /// sessions and cancellation obligations.
    #[must_use]
    pub fn new(
        invitation: Invitation,
        ringing: Ringing,
        dialing: Dialing,
        outgoing_incoming: mpsc::Receiver<Incoming>,
        endpoint: &sipx_transport::Handle,
        media_address: IpAddr,
    ) -> Self {
        Self {
            invitation: invitation.into_coupling(),
            ringing,
            dialing: Some(dialing),
            outgoing_incoming,
            endpoint: endpoint.clone(),
            media_address,
            state: CouplingState::new(),
            deferred: PerLeg::new(VecDeque::new(), VecDeque::new()),
        }
    }

    /// The shared offer/answer and lifecycle policy while either leg remains early.
    #[must_use]
    pub fn state(&self) -> &CouplingState {
        &self.state
    }

    /// Drive both early legs until they confirm, are cancelled, or receive a final refusal.
    ///
    /// A matching inbound CANCEL withdraws the pending outbound INVITE. If its 2xx crossed the
    /// CANCEL, that now-confirmed outbound call receives BYE instead. An outbound 4xx/5xx is sent
    /// as the inbound INVITE's final response with the same status and reason.
    pub async fn confirmed(mut self) -> Result<ConfirmedCoupling> {
        let cancellation = self.invitation.cancellation();
        let mut outbound_call = None;

        loop {
            if outbound_call.is_some()
                && (!self.ringing.has_early_session() || self.ringing.is_acknowledged())
            {
                return Box::pin(self.finish_confirmation(outbound_call.take())).await;
            }
            if let Some(request) = self.deferred.one.pop_front() {
                self.handle_early_request(Leg::One, request, &mut outbound_call)
                    .await?;
                continue;
            }
            if let Some(request) = self.deferred.two.pop_front() {
                self.handle_early_request(Leg::Two, request, &mut outbound_call)
                    .await?;
                continue;
            }

            tokio::select! {
                () = cancellation.cancelled() => {
                    match self.state.cancel(Leg::One) {
                        CancelAction::CancelPeer => {
                            if let Some(dialing) = self.dialing.take() {
                                dialing.cancel().await;
                            }
                        }
                        CancelAction::ByePeer => {
                            if let Some(mut call) = outbound_call {
                                call.hang_up().await?;
                            }
                        }
                        CancelAction::AcknowledgeOnly => {}
                    }
                    return Err(Error::InvitationCancelled);
                }
                request = self.invitation.requests.recv() => {
                    let Some(request) = request else {
                        if let Some(mut call) = outbound_call {
                            call.hang_up().await?;
                        } else if let Some(dialing) = self.dialing.take() {
                            dialing.cancel().await;
                        }
                        return Err(Error::InvitationCancelled);
                    };
                    self.handle_early_request(Leg::One, request, &mut outbound_call).await?;
                }
                step = async {
                    if outbound_call.is_some() {
                        return Ok(CouplingDialEvent::Incoming(Box::new(
                            self.outgoing_incoming.recv().await,
                        )));
                    }
                    let Some(dialing) = self.dialing.as_mut() else {
                        return std::future::pending::<Result<CouplingDialEvent>>().await;
                    };
                    dialing.coupling_step(&mut self.outgoing_incoming).await
                } => {
                    self.handle_dial_event(step, &mut outbound_call).await?;
                }
            }
        }
    }

    async fn handle_dial_event(
        &mut self,
        event: Result<CouplingDialEvent>,
        outbound_call: &mut Option<Call>,
    ) -> Result<()> {
        match event {
            Ok(CouplingDialEvent::Progress) => Ok(()),
            Ok(CouplingDialEvent::Answered(call)) => {
                self.state.confirm(Leg::Two);
                *outbound_call = Some(*call);
                self.dialing = None;
                Ok(())
            }
            Ok(CouplingDialEvent::Incoming(request)) => {
                let Some(request) = *request else {
                    if let Some(mut call) = outbound_call.take() {
                        // discard: the peer inbox has already closed, so there is no caller left
                        // to receive this cleanup failure. The BYE transmit is counted by the
                        // transport; retain the primary `NoResponse` cause and make the secondary
                        // failure observable here.
                        if let Err(error) = call.hang_up().await {
                            tracing::warn!(%error, "could not hang up an orphaned coupled call");
                        }
                    } else if let Some(dialing) = self.dialing.take() {
                        dialing.cancel().await;
                    }
                    self.invitation
                        .refuse(&self.endpoint, 503, "Service Unavailable")
                        .await?;
                    return Err(Error::NoResponse);
                };
                if request.request.method != Method::Bye {
                    return self
                        .handle_early_request(Leg::Two, request, outbound_call)
                        .await;
                }
                let Some(call) = outbound_call.as_mut() else {
                    return Ok(());
                };
                if call.handle(&request).await? && call.is_ended() {
                    self.invitation
                        .refuse(&self.endpoint, 487, "Request Terminated")
                        .await?;
                    return Err(Error::Rejected {
                        status: 487,
                        reason: "Request Terminated".to_owned(),
                    });
                }
                Ok(())
            }
            Err(Error::Rejected { status, reason }) => {
                self.dialing = None;
                self.invitation
                    .refuse(&self.endpoint, status, reason.clone())
                    .await?;
                Err(Error::Rejected { status, reason })
            }
            Err(error) => {
                self.dialing = None;
                self.invitation
                    .refuse(&self.endpoint, 503, "Service Unavailable")
                    .await?;
                Err(error)
            }
        }
    }

    async fn finish_confirmation(self, outbound: Option<Call>) -> Result<ConfirmedCoupling> {
        let Some(mut outbound) = outbound else {
            return Err(Error::NoResponse);
        };
        if let Err(error) = self.invitation.claim() {
            outbound.hang_up().await?;
            return Err(error);
        }
        let inbound = if self.ringing.has_early_session() {
            let mut ringing = self.ringing;
            crate::answer_early(&self.endpoint, &self.invitation.incoming, &mut ringing).await?
        } else {
            crate::answer_ringing(
                &self.endpoint,
                &self.invitation.incoming,
                self.media_address,
                &self.ringing,
            )
            .await?
        };
        Ok(ConfirmedCoupling {
            coupling: Coupling::new(inbound, outbound),
            one_incoming: self.invitation.requests,
            two_incoming: self.outgoing_incoming,
        })
    }

    async fn handle_early_request(
        &mut self,
        leg: Leg,
        incoming: Incoming,
        outbound_call: &mut Option<Call>,
    ) -> Result<()> {
        if leg == Leg::One && incoming.request.method == Method::Prack {
            let _matched = self.ringing.on_prack(&incoming).await?;
            return Ok(());
        }
        if incoming.request.method != Method::Update {
            Call::refuse_with(&self.endpoint, &incoming, 405, "Method Not Allowed").await?;
            return Ok(());
        }
        if !crate::update::carries_offer(&incoming.request) {
            return self
                .handle_offerless_update(leg, &incoming, outbound_call)
                .await;
        }
        self.handle_early_offer(leg, incoming, outbound_call).await
    }

    async fn handle_offerless_update(
        &mut self,
        leg: Leg,
        incoming: &Incoming,
        outbound_call: &mut Option<Call>,
    ) -> Result<()> {
        let handled = match leg {
            Leg::One => self.ringing.on_update(incoming).await?,
            Leg::Two => match outbound_call {
                Some(call) => call.handle(incoming).await?,
                None => match self.dialing.as_mut() {
                    Some(dialing) => dialing.on_update(incoming).await?,
                    None => false,
                },
            },
        };
        if !handled {
            Call::refuse_with(&self.endpoint, incoming, 481, "No Dialog").await?;
        }
        Ok(())
    }

    async fn handle_early_offer(
        &mut self,
        leg: Leg,
        incoming: Incoming,
        outbound_call: &mut Option<Call>,
    ) -> Result<()> {
        match self.state.begin_offer(leg, OfferAxis::Update) {
            OfferAction::Refuse { status } => {
                let reason = if status == 491 {
                    "Request Pending"
                } else {
                    "Server Internal Error"
                };
                return Call::refuse_with(&self.endpoint, &incoming, status, reason).await;
            }
            OfferAction::Relay { .. } => {}
        }

        let direction = relayed_direction(&incoming);
        let (relayed, inbox_closed) = match leg {
            Leg::One => match outbound_call {
                Some(call) => {
                    drive_early_outgoing_offer(
                        call.update(direction),
                        &mut self.state,
                        Leg::Two,
                        &mut self.outgoing_incoming,
                        &mut self.deferred.two,
                        &self.endpoint,
                    )
                    .await?
                }
                None => match self.dialing.as_mut() {
                    Some(dialing) => {
                        drive_early_outgoing_offer(
                            dialing.update(direction),
                            &mut self.state,
                            Leg::Two,
                            &mut self.outgoing_incoming,
                            &mut self.deferred.two,
                            &self.endpoint,
                        )
                        .await?
                    }
                    None => (Err(Error::NoDialog), false),
                },
            },
            Leg::Two => {
                drive_early_outgoing_offer(
                    self.ringing.update(direction),
                    &mut self.state,
                    Leg::One,
                    &mut self.invitation.requests,
                    &mut self.deferred.one,
                    &self.endpoint,
                )
                .await?
            }
        };
        if let Err(error) = relayed {
            let _settled = self.state.fail(leg);
            if let Error::Rejected { status, reason } = error {
                return Call::refuse_with(&self.endpoint, &incoming, status, reason).await;
            }
            return Err(error);
        }

        let handled = match leg {
            Leg::One => self.ringing.on_update(&incoming).await,
            Leg::Two => match outbound_call {
                Some(call) => call.handle(&incoming).await,
                None => match self.dialing.as_mut() {
                    Some(dialing) => dialing.on_update(&incoming).await,
                    None => Err(Error::NoDialog),
                },
            },
        };
        let _settled = match &handled {
            Ok(_) => self.state.complete(leg),
            Err(_) => self.state.fail(leg),
        };
        if !handled? {
            Call::refuse_with(&self.endpoint, &incoming, 481, "No Dialog").await?;
        }
        if inbox_closed {
            return Err(Error::NoResponse);
        }
        Ok(())
    }
}

async fn drive_early_outgoing_offer<F>(
    outgoing: F,
    state: &mut CouplingState,
    far_leg: Leg,
    incoming: &mut mpsc::Receiver<Incoming>,
    deferred: &mut VecDeque<Incoming>,
    responder: &sipx_transport::Handle,
) -> Result<(Result<()>, bool)>
where
    F: std::future::Future<Output = Result<()>>,
{
    tokio::pin!(outgoing);
    let mut inbox_closed = false;
    loop {
        tokio::select! {
            biased;
            received = incoming.recv(), if !inbox_closed && deferred.len() < DEFERRED_CAPACITY => {
                let Some(request) = received else {
                    inbox_closed = true;
                    continue;
                };
                let Some(axis) = offer_axis(&request) else {
                    deferred.push_back(request);
                    continue;
                };
                match state.begin_offer(far_leg, axis) {
                    OfferAction::Refuse { status } => {
                        let reason = if status == 491 {
                            "Request Pending"
                        } else {
                            "Server Internal Error"
                        };
                        Call::refuse_with(responder, &request, status, reason).await?;
                    }
                    OfferAction::Relay { .. } => deferred.push_back(request),
                }
            }
            result = &mut outgoing => return Ok((result, inbox_closed)),
        }
    }
}

/// Two confirmed calls owned and driven as one.
#[derive(Debug)]
pub struct Coupling {
    // Declared first so it drops before either call and releases its session handles first.
    bridge: Option<Bridge>,
    one: Call,
    two: Call,
    state: CouplingState,
    deferred: PerLeg<VecDeque<Incoming>>,
}

impl Coupling {
    /// Take sole ownership of two confirmed calls.
    #[must_use]
    pub fn new(one: Call, two: Call) -> Self {
        let mut state = CouplingState::new();
        state.confirm(Leg::One);
        state.confirm(Leg::Two);
        Self {
            bridge: None,
            one,
            two,
            state,
            deferred: PerLeg::new(VecDeque::new(), VecDeque::new()),
        }
    }

    /// Attach the existing channel-based media bridge.
    ///
    /// Without this call the coupling is signalling-only. Calling it again replaces the bridge
    /// after a renegotiation moved either session.
    pub fn bridge_media(&mut self) -> bool {
        let bridge = Bridge::connect(self.one.media_handle(), self.two.media_handle());
        let transcoding = bridge.is_transcoding();
        self.bridge = Some(bridge);
        transcoding
    }

    /// Whether this coupling currently forwards media.
    #[must_use]
    pub fn has_media_bridge(&self) -> bool {
        self.bridge.is_some()
    }

    /// The offer/answer policy, for an early-dialog driver or application adapter.
    #[must_use]
    pub fn state(&self) -> &CouplingState {
        &self.state
    }

    /// Mutably borrow the offer/answer policy while retaining ownership of both calls.
    pub fn state_mut(&mut self) -> &mut CouplingState {
        &mut self.state
    }

    /// Borrow the calls for inspection without transferring either one out.
    #[must_use]
    pub fn calls(&self) -> (&Call, &Call) {
        (&self.one, &self.two)
    }

    /// Drive both routed inboxes until either dialog ends.
    ///
    /// A BYE is first handled on the receiving leg (including its 200), then mapped to a BYE on
    /// the peer. Closing an inbox is terminal too: returning while leaving its peer alive would
    /// orphan a dialog the coupling still owns.
    pub async fn run(
        &mut self,
        one_incoming: &mut mpsc::Receiver<Incoming>,
        two_incoming: &mut mpsc::Receiver<Incoming>,
    ) -> Result<CouplingEnd> {
        loop {
            if let Some(incoming) = self.deferred.one.pop_front() {
                if let Some(end) = self
                    .handle(Leg::One, incoming, one_incoming, two_incoming)
                    .await?
                {
                    return Ok(end);
                }
                continue;
            }
            if let Some(incoming) = self.deferred.two.pop_front() {
                if let Some(end) = self
                    .handle(Leg::Two, incoming, one_incoming, two_incoming)
                    .await?
                {
                    return Ok(end);
                }
                continue;
            }
            let one_deadline = self.one.session_deadline();
            let two_deadline = self.two.session_deadline();
            tokio::select! {
                incoming = one_incoming.recv() => {
                    let Some(incoming) = incoming else {
                        self.two.hang_up().await?;
                        return Ok(CouplingEnd::InboxClosed(Leg::One));
                    };
                    if let Some(end) = self
                        .handle(Leg::One, incoming, one_incoming, two_incoming)
                        .await?
                    {
                        return Ok(end);
                    }
                }
                incoming = two_incoming.recv() => {
                    let Some(incoming) = incoming else {
                        self.one.hang_up().await?;
                        return Ok(CouplingEnd::InboxClosed(Leg::Two));
                    };
                    if let Some(end) = self
                        .handle(Leg::Two, incoming, one_incoming, two_incoming)
                        .await?
                    {
                        return Ok(end);
                    }
                }
                () = sleep_until(one_deadline) => {
                    if let Err(error) = self.one.on_session_deadline().await {
                        if self.one.is_ended() {
                            // discard: the session-timer failure is the cause returned to the
                            // owner. A failed peer BYE is counted at transport and logged here so
                            // replacing the primary error cannot hide or misclassify it.
                            if let Err(cleanup_error) = self.two.hang_up().await {
                                tracing::warn!(%cleanup_error, "could not clean up coupled peer");
                            }
                        }
                        return Err(error);
                    }
                }
                () = sleep_until(two_deadline) => {
                    if let Err(error) = self.two.on_session_deadline().await {
                        if self.two.is_ended() {
                            // discard: the session-timer failure is the cause returned to the
                            // owner. A failed peer BYE is counted at transport and logged here so
                            // replacing the primary error cannot hide or misclassify it.
                            if let Err(cleanup_error) = self.one.hang_up().await {
                                tracing::warn!(%cleanup_error, "could not clean up coupled peer");
                            }
                        }
                        return Err(error);
                    }
                }
            }
        }
    }

    async fn handle(
        &mut self,
        leg: Leg,
        incoming: Incoming,
        one_incoming: &mut mpsc::Receiver<Incoming>,
        two_incoming: &mut mpsc::Receiver<Incoming>,
    ) -> Result<Option<CouplingEnd>> {
        let is_bye = incoming.request.method == Method::Bye;
        if let Some(axis) = offer_axis(&incoming) {
            return self
                .relay_offer(leg, axis, incoming, one_incoming, two_incoming)
                .await;
        }
        let reason = incoming
            .request
            .headers
            .typed::<Reason>()
            .and_then(std::result::Result::ok)
            .and_then(|reason| reason.0.into_iter().next());
        let (call, peer) = match leg {
            Leg::One => (&mut self.one, &mut self.two),
            Leg::Two => (&mut self.two, &mut self.one),
        };
        if !call.handle(&incoming).await? {
            call.refuse_unclaimed(&incoming).await;
            return Ok(None);
        }
        if !is_bye || !call.is_ended() {
            return Ok(None);
        }
        match reason {
            Some(reason) => peer.hang_up_with_reason(reason).await?,
            None => peer.hang_up().await?,
        }
        Ok(Some(CouplingEnd::Bye(leg)))
    }

    async fn relay_offer(
        &mut self,
        leg: Leg,
        axis: OfferAxis,
        incoming: Incoming,
        one_incoming: &mut mpsc::Receiver<Incoming>,
        two_incoming: &mut mpsc::Receiver<Incoming>,
    ) -> Result<Option<CouplingEnd>> {
        match self.state.begin_offer(leg, axis) {
            OfferAction::Refuse { status } => {
                let reason = if status == 491 {
                    "Request Pending"
                } else {
                    "Server Internal Error"
                };
                self.call(leg).refuse(&incoming, status, reason).await?;
                return Ok(None);
            }
            OfferAction::Relay { .. } => {}
        }

        let direction = relayed_direction(&incoming);
        let (relayed, inbox_closed) = match leg {
            Leg::One => {
                drive_outgoing_offer(
                    &mut self.two,
                    &mut self.state,
                    Leg::Two,
                    axis,
                    direction,
                    two_incoming,
                    &mut self.deferred.two,
                )
                .await?
            }
            Leg::Two => {
                drive_outgoing_offer(
                    &mut self.one,
                    &mut self.state,
                    Leg::One,
                    axis,
                    direction,
                    one_incoming,
                    &mut self.deferred.one,
                )
                .await?
            }
        };

        if let Err(error) = relayed {
            let _settled = self.state.fail(leg);
            if let Error::Rejected { status, reason } = error {
                self.call(leg).refuse(&incoming, status, reason).await?;
                return Ok(None);
            }
            return Err(error);
        }

        let handled = match leg {
            Leg::One => self.one.handle(&incoming).await,
            Leg::Two => self.two.handle(&incoming).await,
        };
        let _settled = match &handled {
            Ok(_) => self.state.complete(leg),
            Err(_) => self.state.fail(leg),
        };
        let handled = handled?;
        if !handled {
            self.call(leg).refuse_unclaimed(&incoming).await;
        }
        if self.bridge.is_some() {
            self.bridge_media();
        }
        if inbox_closed {
            self.call_mut(leg).hang_up().await?;
            return Ok(Some(CouplingEnd::InboxClosed(leg.peer())));
        }
        Ok(None)
    }

    fn call(&self, leg: Leg) -> &Call {
        match leg {
            Leg::One => &self.one,
            Leg::Two => &self.two,
        }
    }

    fn call_mut(&mut self, leg: Leg) -> &mut Call {
        match leg {
            Leg::One => &mut self.one,
            Leg::Two => &mut self.two,
        }
    }
}

/// Drive the outgoing half of a relayed offer while continuing to read that leg's routed inbox.
///
/// The outgoing method mutably borrows `far`, so ordinary requests are deferred until it settles.
/// An offer cannot wait: it collided with the outstanding offer and needs its final 491 while the
/// collision is still real. A cloned endpoint handle can send that response without touching the
/// call's dialog state.
async fn drive_outgoing_offer(
    far: &mut Call,
    state: &mut CouplingState,
    far_leg: Leg,
    axis: OfferAxis,
    direction: Direction,
    incoming: &mut mpsc::Receiver<Incoming>,
    deferred: &mut VecDeque<Incoming>,
) -> Result<(Result<()>, bool)> {
    let responder = far.responder();
    let outgoing = async {
        if axis == OfferAxis::Update {
            far.update(direction).await
        } else {
            far.reinvite(direction).await
        }
    };
    tokio::pin!(outgoing);
    let mut inbox_closed = false;

    loop {
        tokio::select! {
            biased;
            received = incoming.recv(), if !inbox_closed && deferred.len() < DEFERRED_CAPACITY => {
                let Some(request) = received else {
                    inbox_closed = true;
                    continue;
                };
                let Some(incoming_axis) = offer_axis(&request) else {
                    deferred.push_back(request);
                    continue;
                };
                match state.begin_offer(far_leg, incoming_axis) {
                    OfferAction::Refuse { status } => {
                        let reason = if status == 491 {
                            "Request Pending"
                        } else {
                            "Server Internal Error"
                        };
                        Call::refuse_with(&responder, &request, status, reason).await?;
                    }
                    OfferAction::Relay { .. } => deferred.push_back(request),
                }
            }
            result = &mut outgoing => return Ok((result, inbox_closed)),
        }
    }
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

fn relayed_direction(incoming: &Incoming) -> Direction {
    let offered = sipx_sdp::parse(&String::from_utf8_lossy(incoming.request.body()))
        .ok()
        .and_then(|description| {
            description
                .media
                .into_iter()
                .find(|media| media.media == "audio" && !media.is_rejected())
                .map(|media| media.direction())
        })
        .unwrap_or(Direction::SendRecv);
    match offered {
        Direction::SendOnly => Direction::RecvOnly,
        Direction::RecvOnly => Direction::SendOnly,
        Direction::SendRecv => Direction::SendRecv,
        Direction::Inactive => Direction::Inactive,
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn every_offer_axis_uses_two_per_leg_states_and_returns_to_idle() {
        for axis in [
            OfferAxis::InitialInvite,
            OfferAxis::ReliableProvisional,
            OfferAxis::Prack,
            OfferAxis::Update,
            OfferAxis::Reinvite,
        ] {
            let mut state = CouplingState::new();
            assert_eq!(
                state.begin_offer(Leg::One, axis),
                OfferAction::Relay {
                    source: Leg::One,
                    axis
                }
            );
            assert!(!state.is_idle());
            assert!(state.complete(Leg::One));
            assert!(state.is_idle());
        }
    }

    #[test]
    fn glare_is_refused_and_a_new_retry_is_accepted_after_completion() {
        let mut state = CouplingState::new();
        assert!(matches!(
            state.begin_offer(Leg::One, OfferAxis::Reinvite),
            OfferAction::Relay { .. }
        ));
        assert_eq!(
            state.begin_offer(Leg::Two, OfferAxis::Update),
            OfferAction::Refuse { status: 491 }
        );
        assert_eq!(
            state.begin_offer(Leg::Two, OfferAxis::Prack),
            OfferAction::Refuse { status: 491 }
        );
        assert!(state.complete(Leg::One));
        assert_eq!(
            state.begin_offer(Leg::Two, OfferAxis::Update),
            OfferAction::Relay {
                source: Leg::Two,
                axis: OfferAxis::Update
            }
        );
    }

    #[test]
    fn cancel_crosses_only_before_the_peer_is_confirmed() {
        let mut state = CouplingState::new();
        assert_eq!(state.cancel(Leg::One), CancelAction::CancelPeer);
        state.confirm(Leg::Two);
        assert_eq!(state.cancel(Leg::One), CancelAction::ByePeer);
        state.confirm(Leg::One);
        assert_eq!(state.cancel(Leg::One), CancelAction::AcknowledgeOnly);
    }

    #[test]
    fn final_failure_preserves_status_until_the_peer_is_confirmed() {
        let mut state = CouplingState::new();
        assert_eq!(
            state.final_failure(Leg::Two, 486),
            FailureAction::RejectPeer { status: 486 }
        );
        assert_eq!(
            state.final_failure(Leg::Two, 503),
            FailureAction::RejectPeer { status: 503 }
        );
        state.confirm(Leg::One);
        assert_eq!(state.final_failure(Leg::Two, 486), FailureAction::ByePeer);
    }
}
