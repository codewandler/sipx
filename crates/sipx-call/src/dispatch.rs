//! One endpoint's requests, routed to any number of concurrent calls (story `C-4`).
//!
//! An endpoint hands out exactly one [`Receiver<Incoming>`](tokio::sync::mpsc::Receiver). Every
//! request that arrives, for any call in any dialog, comes out of that one stream — so an
//! application holding more than one call had to write its own demultiplexer, and
//! [`serve`](crate::serve) dropped whatever the single call it drove did not claim. Both are
//! ways to lose an ACK, which is the loss that leaks calls: nothing retransmits it once Timer H
//! expires and no timer reaps the dialog it would have completed.
//!
//! This is that demultiplexer, written once. It owns the receiver, routes each request to the
//! call it belongs to, and gives every request that belongs to no call a defined answer instead
//! of silence. The decision table, the counters and the vectors the tests are derived from are
//! in [`docs/specs/call-dispatch.md`](../../../docs/specs/call-dispatch.md).
//!
//! ```no_run
//! # async fn example(endpoint: sipx_transport::Handle,
//! #                  incoming: tokio::sync::mpsc::Receiver<sipx_transport::Incoming>)
//! #     -> Result<(), Box<dyn std::error::Error>> {
//! use std::net::IpAddr;
//! use sipx_call::{Dispatched, Dispatcher, serve};
//! use tokio::task::JoinSet;
//!
//! const MAX_CALLS: usize = 64;
//! let media_address: IpAddr = "203.0.113.7".parse()?;
//! let mut dispatcher = Dispatcher::new(endpoint.clone(), incoming);
//! let mut calls = JoinSet::new();
//! let outcome: Result<(), Box<dyn std::error::Error>> = async {
//!     loop {
//!         tokio::select! {
//!             event = dispatcher.next() => {
//!                 let Some(event) = event else { break };
//!                 if let Dispatched::Invitation(invitation) = event {
//!                     if calls.len() >= MAX_CALLS {
//!                         invitation.refuse(&endpoint, 503, "Service Unavailable").await?;
//!                         continue;
//!                     }
//!                     let mut call = invitation.answer(&endpoint, media_address).await?;
//!                     let (_, mut requests) = invitation.into_parts();
//!                     calls.spawn(async move { serve(&mut call, &mut requests).await });
//!                 }
//!             }
//!             joined = calls.join_next(), if !calls.is_empty() => {
//!                 if let Some(joined) = joined { joined??; }
//!             }
//!         }
//!     }
//!     Ok(())
//! }.await;
//! calls.shutdown().await;
//! outcome
//! # }
//! ```

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use bytes::Bytes;
use sipx_sip::build::ResponseBuilder;
use sipx_sip::transaction::TransactionKey;
use sipx_sip::{HeaderName, Method, Request, StatusCode};
use sipx_transport::{Handle, Incoming};
use tokio::sync::{Notify, mpsc};

use crate::call::{Call, token};
use crate::dialog::{Dialog, cseq_number, from_tag, to_tag};
use crate::error::{Error, Result};
use crate::event::{CallEvents, EndCause, EventSink};
use crate::identity::InboundIdentityPolicy;
use crate::media_policy::{Codecs, MediaPolicy};
use crate::notifier::Notifier;
use crate::subscriber::EventSubscriptions;

/// How many requests one call's inbox holds before the dispatcher sheds for it.
///
/// Deep enough that a call busy with one signalling exchange does not shed the next request
/// behind it, shallow enough that a task which has stopped reading is noticed rather than
/// buffered for. Override with [`Dispatcher::with_queue`].
pub const DEFAULT_QUEUE: usize = 16;

/// The `Retry-After` a shed request carries, in seconds.
///
/// The same value [`sipx_transport`] sheds with, for the same reason: the peer is being told the
/// moment was wrong, not that the call is gone, and a number lets it act on that.
const RETRY_AFTER: &[u8] = b"5";

/// What the dispatcher could not place itself, handed to the application.
#[derive(Debug)]
#[non_exhaustive]
pub enum Dispatched {
    /// An INVITE that belongs to no live call: an incoming call.
    ///
    /// Answering, ringing or rejecting it is the application's decision — the dispatcher takes
    /// none of them. Its inbox is already routed, so nothing that arrives while the application
    /// decides can be missed.
    Invitation(Invitation),
    /// A request outside any dialog whose method this stack advertises but that matched no
    /// route: an OPTIONS ping.
    ///
    /// Surfaced rather than refused because the `Allow` a 405 would carry
    /// ([`sipx_sip::update::ALLOW`]) names it, and answering 405 to a method the same message
    /// says is supported tells the peer two different things at once. What answers an OPTIONS is
    /// a user agent (`sipx_ua::Agent::answer`), which the call framework does not have.
    ///
    /// **A CANCEL never arrives here** (`S-23`). It is the one advertised method the dispatcher
    /// can place itself, because RFC 3261 §9.2 says exactly what to do with it and both halves of
    /// the answer are the dispatcher's to give: the `200 OK` on the CANCEL's own transaction, and
    /// the `487 Request Terminated` on the INVITE transaction it withdraws. One that matches no
    /// pending INVITE transaction is answered `481` rather than handed over, because there is
    /// nothing an application could usefully decide about a transaction this stack does not have.
    OutOfDialog(Incoming),
}

/// An incoming call: the INVITE, and the inbox of the call it may become.
///
/// The inbox exists before the application has decided anything, and that is the point. The ACK
/// to our own 2xx can arrive before `answer` has returned, so a route installed only once a
/// [`Call`] existed would have nowhere to put it.
///
/// Dropping this without answering releases the route: the next request for that dialog is
/// answered as an unknown one rather than queued for a call that will never exist.
///
/// It is also what a CANCEL for this INVITE ends (RFC 3261 §9.2). The dispatcher answers the
/// CANCEL itself — [`is_cancelled`](Self::is_cancelled) and [`events`](Self::events) are how an
/// application finds out, and [`answer`](Self::answer) is how it is stopped from accepting an
/// invitation the caller has already withdrawn.
#[derive(Debug)]
pub struct Invitation {
    incoming: Incoming,
    requests: mpsc::Receiver<Incoming>,
    /// Shared with the dispatcher's table: the INVITE server transaction a CANCEL names.
    pending: Arc<Pending>,
    /// Handed out once by [`Self::events`], as [`Call::events`](crate::Call::events) does.
    events: Option<CallEvents>,
}

impl Invitation {
    /// The INVITE, to answer, ring or refuse.
    #[must_use]
    pub fn request(&self) -> &Incoming {
        &self.incoming
    }

    /// Whether the caller withdrew this invitation before it was answered (RFC 3261 §9.2).
    ///
    /// True from the moment the dispatcher has answered a matching CANCEL, which is also the
    /// moment it sent the `487` that ended the INVITE transaction. There is nothing left to
    /// accept: [`Self::answer`] refuses with [`Error::InvitationCancelled`] from here on.
    ///
    /// This is the poll. [`Self::events`] is the push, and an application that is *ringing* wants
    /// that one — it has to be told to stop, not remember to ask.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.pending.is_cancelled()
    }

    /// This invitation's event stream, handed out exactly once.
    ///
    /// Returns `None` on every call after the first, the same contract
    /// [`Call::events`](crate::Call::events) has and for the same reason: there is one consumer
    /// by construction rather than a value a second reader could be cloned from.
    ///
    /// Exactly one event is ever emitted on it —
    /// [`CallEvent::Ended`](crate::CallEvent::Ended)`(`[`EndCause::RemoteCancel`]`)`, when the
    /// caller gives up. An invitation that is *answered* produces no event here: it becomes a
    /// [`Call`], which has a stream of its own that starts with `Answered`.
    #[must_use]
    pub fn events(&mut self) -> Option<CallEvents> {
        self.events.take()
    }

    /// Answer this invitation, unless the caller has already withdrawn it.
    ///
    /// [`crate::answer`] with two things the free function cannot know, both of which come from
    /// the invitation owning the INVITE's server transaction:
    ///
    /// - It fails with [`Error::InvitationCancelled`] once a CANCEL has ended the transaction,
    ///   rather than putting a `200` on a transaction that already carried a `487`.
    /// - It records that a final response has gone out, which is what makes a CANCEL arriving
    ///   afterwards the no-op RFC 3261 §9.2 requires instead of a teardown.
    ///
    /// The `To` tag is the invitation's own, so the `200` accepting it and the `200` answering a
    /// late CANCEL agree on one, which is §9.2's `SHOULD`.
    ///
    /// Prefer this to [`crate::answer`] on anything a [`Dispatcher`] surfaced. The free function
    /// still works and still answers correctly — it simply cannot tell the dispatcher what it
    /// did, so a CANCEL that arrives around it is judged on the transaction's last known state.
    ///
    /// Answers from the default codec set, [`Codecs::G711`]. [`Self::answer_with`] takes a
    /// selection.
    pub async fn answer(&self, endpoint: &Handle, media_address: IpAddr) -> Result<Call> {
        self.answer_with(endpoint, media_address, Codecs::default())
            .await
    }

    /// [`Self::answer`], from a chosen codec set rather than the default one (`M-30`).
    ///
    /// The dispatcher's counterpart of [`crate::answer_with`]. This exists rather than being left
    /// to the free function because this is the path the docs above tell an application to prefer:
    /// a selection reachable only through [`crate::answer_with`] would be a selection every
    /// dispatched call has to give up cancellation bookkeeping to make.
    pub async fn answer_with(
        &self,
        endpoint: &Handle,
        media_address: IpAddr,
        codecs: Codecs,
    ) -> Result<Call> {
        self.answer_with_policy(
            endpoint,
            media_address,
            MediaPolicy::default().with_codecs(codecs),
        )
        .await
    }

    /// [`Self::answer`], using one coherent codec and ICE policy.
    pub async fn answer_with_policy(
        &self,
        endpoint: &Handle,
        media_address: IpAddr,
        policy: MediaPolicy,
    ) -> Result<Call> {
        self.answer_with_policy_at(endpoint, crate::MediaAddress::new(media_address), policy)
            .await
    }

    /// [`Self::answer_with_policy`] with independent advertised and bound media addresses.
    pub async fn answer_with_policy_at(
        &self,
        endpoint: &Handle,
        media_address: crate::MediaAddress,
        policy: MediaPolicy,
    ) -> Result<Call> {
        // Handed down rather than taken here, so that the invitation is taken immediately before
        // the `200` leaves rather than before the work that builds it — every step of which can
        // fail with nothing sent, and an invitation taken by one of those is one no CANCEL can
        // end. `answer_negotiated` documents the placement.
        crate::call::answer_tagged(
            endpoint,
            &self.incoming,
            media_address,
            self.pending.tag(),
            Some(&|| self.pending.claim()),
            policy,
            &[],
        )
        .await
    }

    /// Refuse this pending invitation with a final response.
    ///
    /// The dispatcher's cancellation state is claimed before the response leaves, so a crossing
    /// CANCEL receives its own 200 but cannot also replace this final response with 487.
    pub async fn refuse(
        &self,
        endpoint: &Handle,
        status: u16,
        reason: impl Into<Bytes>,
    ) -> Result<()> {
        self.pending.claim()?;
        final_response(endpoint, &self.incoming, self.pending.tag(), status, reason).await
    }

    /// Split into the INVITE and the inbox, ready for
    /// [`answer`](crate::answer) and [`serve`](crate::serve).
    ///
    /// What is given up is the cancellation state: [`Self::is_cancelled`] and [`Self::answer`] go
    /// with it. The dispatcher keeps answering CANCELs for this transaction either way — the
    /// table owns that, not this handle — so what is lost is the application's view of it, which
    /// is why this is the call to make *after* answering rather than instead of it.
    #[must_use]
    pub fn into_parts(self) -> (Incoming, mpsc::Receiver<Incoming>) {
        (self.incoming, self.requests)
    }

    /// Transfer this pending invitation to the two-dialog coupling driver.
    pub(crate) fn into_coupling(self) -> CouplingInvitation {
        CouplingInvitation {
            incoming: self.incoming,
            requests: self.requests,
            pending: self.pending,
        }
    }
}

/// The invitation state retained by an early two-dialog coupling.
#[derive(Debug)]
pub(crate) struct CouplingInvitation {
    pub(crate) incoming: Incoming,
    pub(crate) requests: mpsc::Receiver<Incoming>,
    pending: Arc<Pending>,
}

impl CouplingInvitation {
    pub(crate) fn cancellation(&self) -> CouplingCancellation {
        CouplingCancellation(Arc::clone(&self.pending))
    }

    pub(crate) fn claim(&self) -> Result<()> {
        self.pending.claim()
    }

    pub(crate) async fn refuse(
        &self,
        endpoint: &Handle,
        status: u16,
        reason: impl Into<Bytes>,
    ) -> Result<()> {
        self.pending.claim()?;
        final_response(endpoint, &self.incoming, self.pending.tag(), status, reason).await
    }
}

async fn final_response(
    endpoint: &Handle,
    incoming: &Incoming,
    tag: &str,
    status: u16,
    reason: impl Into<Bytes>,
) -> Result<()> {
    let status = StatusCode::new(status).ok_or_else(|| Error::Rejected {
        status,
        reason: "invalid final response status".to_owned(),
    })?;
    let response = ResponseBuilder::to_request(&incoming.request, status, reason)
        .and_then(|builder| with_to_tag(builder, &incoming.request, Some(tag)))?
        .build();
    endpoint.respond(&incoming.key, response).await?;
    Ok(())
}

#[derive(Debug, Clone)]
pub(crate) struct CouplingCancellation(Arc<Pending>);

impl CouplingCancellation {
    pub(crate) async fn cancelled(&self) {
        loop {
            let notified = self.0.cancelled.notified();
            if self.0.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

/// One INVITE server transaction the dispatcher surfaced, and what RFC 3261 §9.2 needs to end it.
///
/// Shared between the [`Invitation`] the application holds and the dispatcher's table, because
/// both have half the question: the dispatcher sees the CANCEL arrive, the application decides
/// whether the invitation was answered first, and §9.2's rule is about which of those happened.
#[derive(Debug)]
struct Pending {
    /// The INVITE's own server transaction — where the `487` goes.
    ///
    /// Not the CANCEL's. The whole difficulty of §9.2 is that the answer is two responses on two
    /// transactions, and a stack that keeps only one key can only ever send one of them.
    transaction: TransactionKey,
    /// The INVITE, which the `487` is built from.
    request: Request,
    /// The `To` tag every response this side sends about this invitation carries (§9.2).
    tag: String,
    /// The route this invitation reserved, kept only so a finished one can be swept.
    ///
    /// A transaction whose call has dropped its inbox is gone as far as anything here is
    /// concerned, and the table would otherwise hold its INVITE for the life of the dispatcher.
    route: mpsc::Sender<Incoming>,
    state: Mutex<State>,
    cancelled: Notify,
}

/// Where an invitation is, and where its one event goes.
///
/// One mutex over both because they change together: the transition to `Cancelled` *is* the
/// emission of `Ended`, and a reader that saw one without the other would see a cancelled
/// invitation nobody was told about, or a report of an end that had not happened yet.
#[derive(Debug)]
struct State {
    phase: Phase,
    events: EventSink,
}

/// The three states RFC 3261 §9.2 distinguishes, and the only three it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// No final response has been sent. A CANCEL ends it.
    Ringing,
    /// A final response has been sent. §9.2: "if it has already sent a final response ... the
    /// CANCEL request has no effect" — BYE is what ends what was answered.
    Answered,
    /// A CANCEL ended it, and the `487` has gone out. It cannot be answered.
    Cancelled,
}

impl Pending {
    fn tag(&self) -> &str {
        &self.tag
    }

    /// The state, whether or not a previous holder panicked.
    ///
    /// The same reasoning as [`Calls::lock`]: the critical section is a field write and a
    /// non-blocking send, so a poisoned lock cannot mean a half-made transition — and refusing to
    /// answer a CANCEL because some unrelated task unwound would be the worse failure.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn is_cancelled(&self) -> bool {
        self.lock().phase == Phase::Cancelled
    }

    /// Take the invitation for a final response of our own.
    ///
    /// Marked *before* the `200` is built rather than after it is sent, and the asymmetry is
    /// deliberate: a CANCEL that arrives mid-answer must not draw a `487` chasing a `200` down
    /// the wire, which is the one ordering that leaves the caller and the callee disagreeing
    /// about whether there is a call. The other way round — an answer that then fails — costs
    /// the caller a CANCEL that says `200` and ends nothing, which its own Timer B resolves.
    fn claim(&self) -> Result<()> {
        let mut state = self.lock();
        if state.phase == Phase::Cancelled {
            return Err(Error::InvitationCancelled);
        }
        state.phase = Phase::Answered;
        Ok(())
    }

    /// End the invitation, if it has not already answered.
    ///
    /// Returns whether the `487` is owed — that is, whether this call was the transition. §9.2
    /// asks for it only "if the transaction for the original request still exists", and both
    /// things that make it not exist come through here: an answer, and an earlier CANCEL whose
    /// retransmission this is.
    fn cancel(&self) -> bool {
        let mut state = self.lock();
        if state.phase != Phase::Ringing {
            return false;
        }
        state.phase = Phase::Cancelled;
        state.events.end(EndCause::RemoteCancel);
        self.cancelled.notify_waiters();
        true
    }
}

/// What a dispatcher has refused, shed or could not place.
///
/// The same shape as [`sipx_transport::ShedCounts`] and for the same reason (`T-19`): loss that
/// cannot be counted from outside is loss nobody is told about.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct DispatchCounts {
    /// Requests answered `503` because the call they belong to was not reading its inbox.
    pub shed: u64,
    /// ACKs that could not be delivered — to a full inbox, or to no call at all.
    ///
    /// Counted apart because an ACK cannot be refused: SIP has no response to one, so nothing
    /// retransmits it after Timer H and the dialog it would have completed is not reaped unless
    /// RFC 4028 session timers happen to be running. This is the one that leaks calls.
    pub acks: u64,
    /// Requests answered `481 Call/Transaction Does Not Exist`.
    ///
    /// Two kinds, counted together because they are one fact — something named a dialog or a
    /// transaction this endpoint does not have: an in-dialog request for no live call (RFC 3261
    /// §12.2.2), and a CANCEL matching no pending INVITE transaction (§9.2).
    pub unmatched: u64,
    /// Out-of-dialog requests answered `405 Method Not Allowed` (RFC 3261 §8.2.1).
    pub unsupported: u64,
    /// Requests answered `400 Bad Request` for naming no dialog at all — no `Call-ID`, or no
    /// `From` tag, both of which RFC 3261 §8.1.1 makes mandatory.
    pub malformed: u64,
    /// INVITEs answered `482 Loop Detected` as merged requests (RFC 3261 §8.2.2.2).
    pub merged: u64,
    /// Initial INVITEs refused by the caller-selected authenticated-identity policy.
    pub identity: u64,
}

impl DispatchCounts {
    /// Everything the dispatcher did not deliver, of every kind.
    ///
    /// Every field above, and every refusal the dispatcher makes is on one of them. That is a
    /// property worth stating rather than assuming: two of these fields exist because the first
    /// version of this type had four, and the `400` and `482` branches moved no counter at all —
    /// two refusals invisible to the counters this story added to make loss visible.
    #[must_use]
    pub fn total(self) -> u64 {
        self.shed
            .saturating_add(self.acks)
            .saturating_add(self.unmatched)
            .saturating_add(self.unsupported)
            .saturating_add(self.malformed)
            .saturating_add(self.merged)
            .saturating_add(self.identity)
    }
}

/// What identifies a route: a `Call-ID` and the tag of the party at the other end.
///
/// The local tag is deliberately absent — see
/// [`docs/specs/call-dispatch.md`](../../../docs/specs/call-dispatch.md) §2. It is what lets a
/// route be reserved from an INVITE, before this side has chosen a tag of its own.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RouteKey {
    call_id: Vec<u8>,
    peer_tag: Vec<u8>,
}

impl RouteKey {
    /// The route an arriving request belongs to, if it names one at all.
    ///
    /// `None` when the `Call-ID` or the `From` tag is missing, which RFC 3261 §8.1.1 makes
    /// mandatory on every request: such a message cannot be placed in any dialog, present or
    /// future.
    fn of(request: &Request) -> Option<Self> {
        Some(Self {
            call_id: request.headers.value(&HeaderName::CallId)?.into_owned(),
            peer_tag: from_tag(&request.headers)?,
        })
    }

    /// The route a call is reached at.
    fn of_dialog(dialog: &Dialog) -> Self {
        Self {
            call_id: dialog.id.call_id.clone(),
            peer_tag: dialog.id.remote_tag.clone(),
        }
    }
}

/// One live route: where a call's requests go, and what reserved it.
#[derive(Debug)]
struct Route {
    tx: mpsc::Sender<Incoming>,
    /// The `CSeq` of the INVITE this route was reserved from, when it was reserved from one.
    ///
    /// Kept because RFC 3261 §8.2.2.2 makes a merged request one whose `From` tag, `Call-ID`
    /// **and `CSeq`** all match — the third term is what tells a request arriving twice by two
    /// paths from the ordinary retry of §8.1.3.5, which keeps the first two and increments this.
    /// `None` for a route registered from a dialog that already exists ([`Calls::register`]),
    /// which no out-of-dialog INVITE can be a merged copy of.
    invite_cseq: Option<u32>,
}

/// The routing table, and the counters that describe what missed it.
#[derive(Debug)]
struct Table {
    routes: Mutex<Routing>,
    counts: Counters,
    queue: usize,
}

/// Two indexes over the same set of calls, under one lock.
///
/// They answer different questions and are keyed differently on purpose. `by_dialog` answers
/// "which call does this request belong to", which SIP keys on the dialog. `invites` answers "which
/// transaction does this CANCEL name", which RFC 3261 §9.2 keys on the transaction and nothing
/// else. Serving the second from the first would mean matching a CANCEL by `Call-ID`, which §9.2
/// does not do — see [`Dispatcher::cancel`].
#[derive(Debug, Default)]
struct Routing {
    /// Where each live call's in-dialog requests go.
    by_dialog: HashMap<RouteKey, Route>,
    /// Every INVITE server transaction this dispatcher has surfaced and not yet swept, keyed by
    /// [`TransactionKey::for_cancelled_invite`]'s answer for the CANCEL that would name it.
    invites: HashMap<TransactionKey, Arc<Pending>>,
}

#[derive(Debug, Default)]
struct Counters {
    shed: AtomicU64,
    acks: AtomicU64,
    unmatched: AtomicU64,
    unsupported: AtomicU64,
    malformed: AtomicU64,
    merged: AtomicU64,
    identity: AtomicU64,
}

/// Which counter a request that was not delivered belongs on.
#[derive(Debug, Clone, Copy)]
enum Kind {
    Shed,
    Ack,
    Unmatched,
    Unsupported,
    Malformed,
    Merged,
    Identity,
}

/// The set of calls a dispatcher routes to: a cheap, cloneable handle to its routing table.
///
/// Needed because the dispatcher's loop and the code that places outbound calls are not the same
/// task. A call made with [`dial`](crate::dial) is registered through this from wherever it was
/// dialled; an inbound one needs nothing, because [`Dispatcher::next`] reserved its route before
/// the application ever saw the INVITE.
#[derive(Debug, Clone)]
pub struct Calls(Arc<Table>);

impl Calls {
    /// Route this dialog's in-dialog requests to the returned inbox.
    ///
    /// Hand the inbox to [`serve`](crate::serve). Dropping it — which is what ending a call and
    /// returning from `serve` does — releases the route.
    ///
    /// There is a window on the outbound path this cannot close: [`dial`](crate::dial) returns
    /// only once the 2xx has arrived, so a BYE that overtakes it is answered `481`. Closing it
    /// needs the `Call-ID` to be known before the INVITE is sent, which is a change to `dial`
    /// this story does not make.
    ///
    /// Registering a dialog that already has a route replaces it, and the previous inbox stops
    /// receiving.
    pub fn register(&self, dialog: &Dialog) -> mpsc::Receiver<Incoming> {
        self.install(RouteKey::of_dialog(dialog), None).1
    }

    /// Stop routing to this dialog.
    ///
    /// Rarely needed — dropping the inbox does the same thing lazily — but explicit when an
    /// application tears a call down without dropping the receiver it was serving from.
    pub fn forget(&self, dialog: &Dialog) {
        self.lock().by_dialog.remove(&RouteKey::of_dialog(dialog));
    }

    /// How many calls are currently routed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().by_dialog.len()
    }

    /// Whether no call is routed at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// What has been refused, shed or left unplaced.
    #[must_use]
    pub fn counts(&self) -> DispatchCounts {
        let counts = &self.0.counts;
        DispatchCounts {
            shed: counts.shed.load(Ordering::Relaxed),
            acks: counts.acks.load(Ordering::Relaxed),
            unmatched: counts.unmatched.load(Ordering::Relaxed),
            unsupported: counts.unsupported.load(Ordering::Relaxed),
            malformed: counts.malformed.load(Ordering::Relaxed),
            merged: counts.merged.load(Ordering::Relaxed),
            identity: counts.identity.load(Ordering::Relaxed),
        }
    }

    /// Record one request the dispatcher did not deliver.
    ///
    /// One method per kind rather than a field reached through from the dispatcher, because the
    /// counters exist so that loss is visible and a caller that had to name a path to reach them
    /// is a caller that can quietly name the wrong one.
    fn counted(&self, kind: Kind) {
        let counts = &self.0.counts;
        match kind {
            Kind::Shed => &counts.shed,
            Kind::Ack => &counts.acks,
            Kind::Unmatched => &counts.unmatched,
            Kind::Unsupported => &counts.unsupported,
            Kind::Malformed => &counts.malformed,
            Kind::Merged => &counts.merged,
            Kind::Identity => &counts.identity,
        }
        .fetch_add(1, Ordering::Relaxed);
    }

    /// The table, whether or not a previous holder panicked.
    ///
    /// Every critical section here is a map operation with no `await` and nothing fallible in
    /// it, so a poisoned lock cannot mean a half-updated table — and refusing to route the rest
    /// of an endpoint's calls because one unrelated task unwound would be a far worse failure
    /// than the one poisoning guards against.
    fn lock(&self) -> MutexGuard<'_, Routing> {
        self.0.routes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The sender for a route, if there is one. Cloned rather than borrowed so the lock is not
    /// held across the send that follows.
    fn sender(&self, key: &RouteKey) -> Option<mpsc::Sender<Incoming>> {
        self.lock().by_dialog.get(key).map(|route| route.tx.clone())
    }

    /// The INVITE transaction a CANCEL names, if this dispatcher has it (RFC 3261 §9.2).
    ///
    /// Cloned out of the table rather than borrowed, because answering a CANCEL takes two `await`
    /// points and holding a `std::sync::Mutex` across one of those is how a routing table stops
    /// routing.
    fn pending_invite(&self, key: &TransactionKey) -> Option<Arc<Pending>> {
        self.lock().invites.get(key).map(Arc::clone)
    }

    /// Reserve a route for an invitation that does not exist yet, and remember its INVITE
    /// transaction so a CANCEL naming it can be answered (RFC 3261 §9.2).
    ///
    /// The transaction is keyed by [`TransactionKey::from_request`] over the *received* INVITE
    /// rather than by [`Incoming::key`], so that both sides of the eventual comparison are derived
    /// from a request the transport has already applied `received` and `rport` to. `Incoming::key`
    /// is kept too, but for responding rather than for matching.
    fn reserve(
        &self,
        key: RouteKey,
        incoming: &Incoming,
    ) -> (mpsc::Receiver<Incoming>, Arc<Pending>, CallEvents) {
        let (tx, rx) = self.install(key, cseq_number(&incoming.request.headers));
        let (events, stream) = EventSink::new();
        let pending = Arc::new(Pending {
            transaction: incoming.key.clone(),
            request: incoming.request.clone(),
            tag: token(),
            route: tx,
            state: Mutex::new(State {
                phase: Phase::Ringing,
                events,
            }),
            cancelled: Notify::new(),
        });
        if let Some(matched) = TransactionKey::from_request(&incoming.request) {
            self.lock().invites.insert(matched, Arc::clone(&pending));
        }
        (rx, pending, stream)
    }

    /// Put a route in the table, replacing whatever was there, and sweep the dead on the way.
    ///
    /// A call that has ended dropped its inbox, so its route is dead weight until something
    /// arrives for it. Sweeping on the operations that are already taking the lock keeps a
    /// long-lived dispatcher from accumulating them.
    ///
    /// An INVITE transaction is swept with the route it reserved, and only then: a CANCEL for an
    /// invitation that has been answered still has to draw the `200` of RFC 3261 §9.2, and that
    /// invitation is long past being a `Dispatched::Invitation` by the time it arrives.
    fn install(
        &self,
        key: RouteKey,
        invite_cseq: Option<u32>,
    ) -> (mpsc::Sender<Incoming>, mpsc::Receiver<Incoming>) {
        let (tx, rx) = mpsc::channel(self.0.queue);
        let mut routing = self.lock();
        routing.by_dialog.retain(|_, route| !route.tx.is_closed());
        routing
            .invites
            .retain(|_, pending| !pending.route.is_closed());
        routing.by_dialog.insert(
            key,
            Route {
                tx: tx.clone(),
                invite_cseq,
            },
        );
        (tx, rx)
    }

    fn remove(&self, key: &RouteKey) {
        self.lock().by_dialog.remove(key);
    }

    /// Whether this INVITE is a merged copy of one already accepted (RFC 3261 §8.2.2.2).
    ///
    /// All three of the section's terms, and no fewer. `Call-ID` and the `From` tag are the route
    /// key; the `CSeq` is what separates the same request arriving twice by two paths — which is
    /// what §8.2.2.2 is about — from the retry of §8.1.3.5, which keeps both of those and
    /// increments the `CSeq`. That retry is the ordinary answer to a 401, 407, 413, 415, 420 or
    /// 484, and RFC 4028 §7.3's 422; refusing it 482 would mean a challenged call could never be
    /// placed at all, including by sipx's own UAC, which retries in exactly that shape.
    ///
    /// A **closed** route is not a match either, whatever its `CSeq`. The application dropped
    /// that inbox, which is what refusing an invitation does, so there is no accepted request
    /// left for a second copy to be merged with — and treating one as live would let a refused
    /// invitation poison its key for every later attempt from the same peer.
    fn is_merged(&self, key: &RouteKey, cseq: Option<u32>) -> bool {
        let routing = self.lock();
        routing.by_dialog.get(key).is_some_and(|route| {
            !route.tx.is_closed() && cseq.is_some() && route.invite_cseq == cseq
        })
    }
}

/// One endpoint's incoming requests, routed to any number of concurrent calls.
///
/// Owns the endpoint's `Receiver<Incoming>`, which is the whole reason it can promise anything:
/// there is exactly one of those, so anything else reading it would be reading requests this
/// cannot then route.
#[derive(Debug)]
pub struct Dispatcher {
    endpoint: Handle,
    incoming: mpsc::Receiver<Incoming>,
    calls: Calls,
    identity: Option<InboundIdentityPolicy>,
    notifier: Option<Notifier>,
    event_subscriptions: Option<EventSubscriptions>,
}

impl Dispatcher {
    /// Take over an endpoint's incoming requests, with the default per-call queue depth.
    #[must_use]
    pub fn new(endpoint: Handle, incoming: mpsc::Receiver<Incoming>) -> Self {
        Self::with_queue(endpoint, incoming, DEFAULT_QUEUE)
    }

    /// The same, with a per-call queue depth of your own.
    ///
    /// A depth of zero is raised to one: an unbuffered channel would shed every request that
    /// arrived while the call was between `recv` calls, which is most of them.
    #[must_use]
    pub fn with_queue(endpoint: Handle, incoming: mpsc::Receiver<Incoming>, queue: usize) -> Self {
        Self {
            endpoint,
            incoming,
            calls: Calls(Arc::new(Table {
                routes: Mutex::new(Routing::default()),
                counts: Counters::default(),
                queue: queue.max(1),
            })),
            identity: None,
            notifier: None,
            event_subscriptions: None,
        }
    }

    /// Verify new inbound INVITEs before they become answerable application invitations.
    ///
    /// A verification failure is sent on the INVITE transaction with its RFC 8224 status and the
    /// request is not surfaced. With no selected policy, dispatch remains wire-compatible and
    /// performs no credential acquisition or time read.
    #[must_use]
    pub fn with_identity(mut self, identity: InboundIdentityPolicy) -> Self {
        self.identity = Some(identity);
        self
    }

    /// Serve inbound RFC 6665 SUBSCRIBE requests through this bounded notifier.
    #[must_use]
    pub fn with_notifier(mut self, mut notifier: Notifier) -> Self {
        notifier.attach(self.endpoint.clone());
        self.notifier = Some(notifier);
        self
    }

    /// Route inbound NOTIFY requests to a bounded outbound event-subscription client.
    #[must_use]
    pub fn with_event_subscriptions(mut self, subscriptions: EventSubscriptions) -> Self {
        subscriptions.attach(self.endpoint.clone());
        self.event_subscriptions = Some(subscriptions);
        self
    }

    /// A handle for registering calls this dispatcher did not itself surface.
    #[must_use]
    pub fn calls(&self) -> Calls {
        self.calls.clone()
    }

    /// Route this dialog's requests to the returned inbox — [`Calls::register`], for an
    /// application that holds the dispatcher directly.
    pub fn register(&self, dialog: &Dialog) -> mpsc::Receiver<Incoming> {
        self.calls.register(dialog)
    }

    /// What has been refused, shed or left unplaced.
    #[must_use]
    pub fn counts(&self) -> DispatchCounts {
        self.calls.counts()
    }

    /// The next thing the dispatcher cannot place itself.
    ///
    /// Routes everything else on the way, so this must be called in a loop for a dispatcher to
    /// do its job at all — the ACKs and BYEs of every call it has already handed out move only
    /// while it is being polled. `None` once the endpoint has shut down.
    pub async fn next(&mut self) -> Option<Dispatched> {
        loop {
            let incoming = self.incoming.recv().await?;
            if let Some(surfaced) = self.route(incoming).await {
                return Some(surfaced);
            }
        }
    }

    /// Place one request: the decision table of
    /// [`docs/specs/call-dispatch.md`](../../../docs/specs/call-dispatch.md) §3, in order.
    async fn route(&mut self, incoming: Incoming) -> Option<Dispatched> {
        let Some(key) = RouteKey::of(&incoming.request) else {
            // RFC 3261 §8.1.1 makes `Call-ID` and the `From` tag mandatory on every request.
            // Without them this cannot be placed in any dialog, now or later.
            self.calls.counted(Kind::Malformed);
            self.refuse(&incoming, 400, "Bad Request", None).await;
            return None;
        };

        // An INVITE with no `To` tag is a new call, and has to be recognised as one *before* the
        // route lookup: routed by key alone it would land in an existing call, whose
        // `Dialog::matches` would then reject it and leave nothing to answer it.
        if incoming.request.method == Method::Invite && to_tag(&incoming.request.headers).is_none()
        {
            let cseq = cseq_number(&incoming.request.headers);
            if self.calls.is_merged(&key, cseq) {
                // RFC 3261 §8.2.2.2, all three of its terms: the same `Call-ID`, `From` tag *and*
                // `CSeq` as a request already accepted here, which means this copy reached us by
                // a second path. A retransmission never gets this far — the server transaction
                // absorbs those — so a match here is always a different branch.
                self.calls.counted(Kind::Merged);
                self.refuse(&incoming, 482, "Loop Detected", None).await;
                return None;
            }
            let verification = self
                .identity
                .as_mut()
                .map(|identity| identity.verify(&incoming.request));
            if let Some(Err(failure)) = verification {
                self.calls.counted(Kind::Identity);
                self.refuse(&incoming, failure.status(), failure.reason(), None)
                    .await;
                return None;
            }
            // Anything else is a fresh call attempt, and that includes the §8.1.3.5 retry that
            // follows a 401, 407, 413, 415, 420, 484 or RFC 4028 §7.3's 422 — same `Call-ID` and
            // `From` tag, one higher `CSeq`. It reserves the key afresh, replacing a route whose
            // invitation has been answered and abandoned; anything still holding that inbox stops
            // receiving, which is what it already was.
            let (requests, pending, events) = self.calls.reserve(key, &incoming);
            return Some(Dispatched::Invitation(Invitation {
                incoming,
                requests,
                pending,
                events: Some(events),
            }));
        }

        // Before the route lookup, because a CANCEL does not belong to a *dialog* — it belongs to
        // the INVITE transaction whose branch it carries (RFC 3261 §9.1), which may well be an
        // invitation nobody has answered and so a call that does not exist yet. Routing it by key
        // would put it in an inbox where the two responses §9.2 owes could not be sent from.
        if incoming.request.method == Method::Cancel {
            self.cancel(&incoming).await;
            return None;
        }

        // SUBSCRIBE owns a dialog of its own and therefore cannot be routed by the call table.
        // A tagged refresh is matched inside the notifier against its subscription dialog.
        if incoming.request.method == Method::Subscribe
            && let Some(notifier) = self.notifier.as_mut()
        {
            notifier.receive(&incoming).await;
            return None;
        }

        // NOTIFY owns the subscription dialog established by an outbound SUBSCRIBE, not an INVITE
        // dialog in the call table. The event client validates its exact tags, Event and CSeq.
        if incoming.request.method == Method::Notify
            && let Some(subscriptions) = &self.event_subscriptions
            && subscriptions.receive(&incoming).await
        {
            return None;
        }

        if let Some(sender) = self.calls.sender(&key) {
            self.deliver(&key, sender, incoming).await;
            return None;
        }

        match incoming.request.method {
            // Nothing to answer: SIP has no response to an ACK, and an ACK for a 2xx is a
            // transaction of its own (RFC 3261 §17.1.1.3). Counted and logged instead, because
            // a stray one means a dialog somewhere completed against a call that is not here.
            Method::Ack => {
                self.calls.counted(Kind::Ack);
                tracing::warn!(
                    source = %incoming.source,
                    "an ACK arrived for no live call and cannot be refused"
                );
                None
            }
            // RFC 3261 §12.2.2. Either it says it is in a dialog, or its method exists only
            // inside one — an orphan of a dialog that is gone, not an invitation to open a new
            // exchange.
            ref method if to_tag(&incoming.request.headers).is_some() || dialog_only(method) => {
                self.calls.counted(Kind::Unmatched);
                self.refuse(&incoming, 481, "Call/Transaction Does Not Exist", None)
                    .await;
                None
            }
            // On the `Allow` the 405 below would carry, so refusing it there would have one
            // message say two different things. The application decides.
            ref method if advertised(method) => Some(Dispatched::OutOfDialog(incoming)),
            // RFC 3261 §8.2.1: "the UAS MUST generate a 405 ... and MUST add an Allow header
            // field".
            _ => {
                self.calls.counted(Kind::Unsupported);
                self.refuse(
                    &incoming,
                    405,
                    "Method Not Allowed",
                    Some((
                        HeaderName::Allow,
                        Bytes::from_static(sipx_sip::update::ALLOW.as_bytes()),
                    )),
                )
                .await;
                None
            }
        }
    }

    /// Answer a CANCEL — both halves of RFC 3261 §9.2, or the 481 that says there was nothing to
    /// cancel.
    ///
    /// **The matching is §9.2's and not an approximation of it.** A CANCEL names the transaction
    /// it withdraws by carrying that request's topmost `Via` branch (§9.1), so the match is the
    /// server transaction match of §17.2.3 — branch, sent-by, and the method of the transaction
    /// being cancelled — which is exactly what
    /// [`TransactionKey::for_cancelled_invite`] builds. In particular the `Call-ID` is *not* part
    /// of it: a table keyed on that would answer a CANCEL for the wrong branch of a dialog it
    /// happens to know, which is a stack that stops ringing when it should not have.
    ///
    /// The answer is two responses on two transactions, and that is the part that is easy to get
    /// half-right:
    ///
    /// 1. `200 OK` on the CANCEL's own transaction — a MUST, and unconditional. It is sent even
    ///    when the INVITE has already been answered, because it says "I got your CANCEL", not "I
    ///    stopped".
    /// 2. `487 Request Terminated` on the INVITE transaction it withdraws, and only "if the
    ///    transaction for the original request still exists". A final response already sent means
    ///    it does not, and §9.2 is explicit that the CANCEL then has no effect — BYE is the
    ///    request for ending a call that was answered.
    ///
    /// Both carry the invitation's `To` tag, which is §9.2's `SHOULD` that the two agree.
    ///
    /// One term is added to §9.2's own, and it is §9.1's rather than an invention: a CANCEL "MUST
    /// have the same `Call-ID`, `To`, `From` and `CSeq` as the INVITE", so one whose dialog
    /// identifiers disagree with the transaction its branch names cannot be a legitimate CANCEL
    /// for it. Every well-formed CANCEL passes the check, which is what makes it free; what it
    /// costs an off-path attacker is that guessing or observing a branch is no longer enough to
    /// stop somebody else's phone ringing, since the sent-by in a `Via` is the attacker's to
    /// write.
    async fn cancel(&self, incoming: &Incoming) {
        let matched = TransactionKey::for_cancelled_invite(&incoming.request)
            .and_then(|key| self.calls.pending_invite(&key))
            .filter(|pending| RouteKey::of(&pending.request) == RouteKey::of(&incoming.request));
        let Some(pending) = matched else {
            // §9.2: "If the UAS did not find a matching transaction for the CANCEL according to
            // the procedure above, it SHOULD respond to the CANCEL with a 481." Not dropped, and
            // not the 405 an unadvertised method would draw — the method is fine, the transaction
            // is the thing that is not here.
            self.calls.counted(Kind::Unmatched);
            self.refuse(incoming, 481, "Call/Transaction Does Not Exist", None)
                .await;
            return;
        };

        self.answer_request(
            &incoming.key,
            &incoming.request,
            200,
            "OK",
            None,
            Some(pending.tag()),
        )
        .await;

        if pending.cancel() {
            self.answer_request(
                &pending.transaction,
                &pending.request,
                487,
                "Request Terminated",
                None,
                Some(pending.tag()),
            )
            .await;
        }
    }

    /// Hand a request to the call it belongs to, or say why it could not be.
    ///
    /// Never awaits room. The whole promise of a per-call queue is that one application task
    /// which has stopped reading cannot stop the loop that serves every other call, and a
    /// dispatcher that blocked here would trade a shed request for every call on the endpoint.
    async fn deliver(&self, key: &RouteKey, sender: mpsc::Sender<Incoming>, incoming: Incoming) {
        let is_ack = incoming.request.method == Method::Ack;
        match sender.try_send(incoming) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(incoming)) => {
                if is_ack {
                    self.calls.counted(Kind::Ack);
                    tracing::error!(
                        source = %incoming.source,
                        "a call's queue is full; an ACK was dropped and cannot be refused — \
                         the dialog it would have completed will not be reaped"
                    );
                } else {
                    self.calls.counted(Kind::Shed);
                    tracing::warn!(
                        source = %incoming.source,
                        method = %incoming.request.method,
                        "a call's queue is full; refusing the transaction for that call"
                    );
                    self.refuse(
                        &incoming,
                        503,
                        "Service Unavailable",
                        Some((HeaderName::RetryAfter, Bytes::from_static(RETRY_AFTER))),
                    )
                    .await;
                }
            }
            Err(mpsc::error::TrySendError::Closed(incoming)) => {
                // The application dropped the inbox, which is what ending a call does. The
                // route is stale, so it goes, and the request gets the answer any request for a
                // dialog this endpoint does not have gets.
                self.calls.remove(key);
                if is_ack {
                    self.calls.counted(Kind::Ack);
                } else {
                    self.calls.counted(Kind::Unmatched);
                    self.refuse(&incoming, 481, "Call/Transaction Does Not Exist", None)
                        .await;
                }
            }
        }
    }

    /// Answer a request the dispatcher will not route.
    ///
    /// Failures are logged rather than returned: this is the path that exists so that nothing is
    /// dropped in silence, and giving it an error for the caller to ignore would put the silence
    /// back one level up.
    async fn refuse(
        &self,
        incoming: &Incoming,
        status: u16,
        reason: &'static str,
        extra: Option<(HeaderName, Bytes)>,
    ) {
        self.answer_request(
            &incoming.key,
            &incoming.request,
            status,
            reason,
            extra,
            None,
        )
        .await;
    }

    /// Answer a request on a named transaction, with a named `To` tag.
    ///
    /// Separate from [`Self::refuse`] because RFC 3261 §9.2 needs both of the things that method
    /// takes for granted to be chosen: the `487` goes on the *INVITE's* transaction rather than
    /// the one the request in hand arrived on, and both of the responses it owes carry the
    /// invitation's tag rather than a fresh one each.
    async fn answer_request(
        &self,
        key: &TransactionKey,
        request: &Request,
        status: u16,
        reason: &'static str,
        extra: Option<(HeaderName, Bytes)>,
        tag: Option<&str>,
    ) {
        let Some(code) = StatusCode::new(status) else {
            return;
        };
        let built = ResponseBuilder::to_request(request, code, reason)
            .and_then(|builder| match extra {
                Some((name, value)) => builder.header(name, value),
                None => Ok(builder),
            })
            .and_then(|builder| with_to_tag(builder, request, tag));
        let response = match built {
            Ok(builder) => builder.build(),
            // discard: the refusal this dispatcher decided on is lost. **It reaches no counter**,
            // and saying so is the point — `Calls::counted` has already recorded the *decision*
            // (which kind of refusal was owed), so `DispatchCounts` will show the request as
            // refused when in fact nothing was sent. The two numbers can therefore disagree with
            // the wire, and an operator should know that before using them to rule a cause out
            // (§12.2). The peer retransmits and its own transaction times out, so nothing hangs.
            Err(error) => {
                tracing::warn!(%error, status, "could not build the response for a request");
                return;
            }
        };
        // discard: the same loss one step later and with the same gap — see above. Closing it
        // needs a counter for responses the endpoint could not send, which belongs with
        // `sipx_transport::Handle::respond` rather than here.
        if let Err(error) = self.endpoint.respond(key, response).await {
            tracing::warn!(%error, status, "could not send the response for a request");
        }
    }
}

/// Give a response a `To` tag if the request did not already carry one.
///
/// RFC 3261 §8.2.6.2: every response but a 100 must have one, and an out-of-dialog request
/// arrives without it. A refusal with no tag is a response a peer is entitled to discard, which
/// would turn a considered answer back into the silence this whole path exists to remove.
///
/// `tag` names one rather than minting one. Only §9.2's pair of responses passes it: the `200` for
/// a CANCEL and the `487` for the INVITE it withdraws are two responses about one invitation, and
/// the section asks that they carry the same tag. Everything else is a one-off refusal with no
/// second response to agree with, and takes a fresh token.
pub(crate) fn with_to_tag(
    builder: sipx_sip::build::ResponseBuilder,
    request: &Request,
    tag: Option<&str>,
) -> std::result::Result<sipx_sip::build::ResponseBuilder, sipx_sip::error::BuildError> {
    if to_tag(&request.headers).is_some() {
        return Ok(builder);
    }
    let Some(to) = request.headers.value(&HeaderName::To) else {
        return Ok(builder);
    };
    let tag = tag.map_or_else(token, str::to_owned);
    // Appending works in both forms of the header: after `>` in a name-addr, and after a bare
    // addr-spec, where the semicolon starts a header parameter (RFC 3261 §20).
    let value = format!("{};tag={tag}", String::from_utf8_lossy(&to));
    builder.set_header(&HeaderName::To, Bytes::from(value))
}

/// Whether a method is defined only inside a dialog.
///
/// One arriving without a `To` tag is therefore an orphan of a dialog that is gone rather than a
/// new exchange, and RFC 3261 §12.2.2's 481 is the honest answer. Listing them is narrower than
/// the alternative of treating every unrecognised out-of-dialog request as an orphan, which
/// would answer 481 to things that are simply unsupported.
pub(crate) fn dialog_only(method: &Method) -> bool {
    matches!(
        method,
        Method::Bye
            | Method::Update
            | Method::Prack
            | Method::Refer
            | Method::Notify
            | Method::Info
    )
}

/// Whether this stack advertises the method (RFC 3311 §4, RFC 3261 §20.5).
///
/// Read from [`sipx_sip::update::ALLOW`] rather than written out again, because that constant is
/// what a 405 puts on the wire and what a peer reads as permission. A second copy that drifted
/// would have one message advertise a method the next refuses.
fn advertised(method: &Method) -> bool {
    let token = method.as_bytes();
    sipx_sip::update::ALLOW
        .split(',')
        .any(|allowed| allowed.trim().as_bytes().eq_ignore_ascii_case(token))
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
    use sipx_sip::{Limits, Message, parse_datagram};

    fn request(text: &str) -> Request {
        match parse_datagram(Bytes::from(text.to_owned()), &Limits::datagram()).expect("parses") {
            Message::Request(r) => r,
            Message::Response(_) => panic!("a request"),
        }
    }

    fn bye(call_id: &str, from_tag: &str, to_tag: &str) -> Request {
        request(&format!(
            "BYE sip:callee@192.0.2.9:5060 SIP/2.0\r\n\
             Via: SIP/2.0/UDP 192.0.2.1:5060;branch=z9hG4bKbye\r\n\
             To: <sip:callee@example.com>;tag={to_tag}\r\n\
             From: <sip:caller@example.net>;tag={from_tag}\r\n\
             Call-ID: {call_id}\r\n\
             CSeq: 2 BYE\r\n\
             Max-Forwards: 70\r\n\
             Content-Length: 0\r\n\r\n"
        ))
    }

    /// The key is the `Call-ID` and the peer's tag, and *not* ours — which is what lets a route
    /// be reserved from an INVITE, before this side has chosen a tag at all.
    #[test]
    fn a_route_is_keyed_without_our_own_tag() {
        let first = RouteKey::of(&bye("c@sipx", "theirs", "ours")).expect("a key");
        let second = RouteKey::of(&bye("c@sipx", "theirs", "a-different-local-tag")).expect("key");
        assert_eq!(first, second, "our own tag must not enter the key");

        let other_call = RouteKey::of(&bye("other@sipx", "theirs", "ours")).expect("a key");
        assert_ne!(first, other_call);
        let other_peer = RouteKey::of(&bye("c@sipx", "someone-else", "ours")).expect("a key");
        assert_ne!(first, other_peer, "a different peer is a different route");
    }

    /// RFC 3261 §8.1.1 makes both parts mandatory. A request missing either cannot be placed in
    /// any dialog, and the dispatcher answers 400 rather than inventing a route for it.
    #[test]
    fn a_request_that_names_no_dialog_has_no_key() {
        let tagless = request(
            "BYE sip:callee@192.0.2.9:5060 SIP/2.0\r\n\
             Via: SIP/2.0/UDP 192.0.2.1:5060;branch=z9hG4bKbye\r\n\
             To: <sip:callee@example.com>;tag=ours\r\n\
             From: <sip:caller@example.net>\r\n\
             Call-ID: c@sipx\r\n\
             CSeq: 2 BYE\r\n\
             Max-Forwards: 70\r\n\
             Content-Length: 0\r\n\r\n",
        );
        assert!(RouteKey::of(&tagless).is_none(), "no From tag, no route");
    }

    /// The 405's `Allow` and this predicate must be the same list, or one message advertises a
    /// method the next refuses. Reading the constant is what makes that structural.
    #[test]
    fn what_is_advertised_is_exactly_the_allow_constant() {
        for method in [
            Method::Invite,
            Method::Ack,
            Method::Cancel,
            Method::Bye,
            Method::Options,
            Method::Update,
        ] {
            assert!(
                advertised(&method),
                "{method} is on ALLOW but not advertised"
            );
        }
        for method in [
            Method::Register,
            Method::Subscribe,
            Method::Publish,
            Method::Message,
            Method::Other(Bytes::from_static(b"FROBNICATE")),
        ] {
            assert!(!advertised(&method), "{method} is not on ALLOW");
        }
    }

    /// A token, not a substring: `UPDATEX` is a different method, and a substring test would
    /// advertise it.
    #[test]
    fn advertising_matches_tokens_and_not_substrings() {
        assert!(!advertised(&Method::Other(Bytes::from_static(b"UPDATEX"))));
        assert!(!advertised(&Method::Other(Bytes::from_static(b"INV"))));
    }

    /// The methods that only exist inside a dialog get 481 when no call claims them; the rest
    /// are either surfaced or refused 405, which are different answers to different questions.
    #[test]
    fn the_dialog_only_methods_are_the_ones_that_orphan() {
        for method in [
            Method::Bye,
            Method::Update,
            Method::Prack,
            Method::Refer,
            Method::Notify,
            Method::Info,
        ] {
            assert!(dialog_only(&method), "{method} exists only in a dialog");
        }
        for method in [Method::Invite, Method::Options, Method::Register] {
            assert!(!dialog_only(&method), "{method} can start something");
        }
    }
}
