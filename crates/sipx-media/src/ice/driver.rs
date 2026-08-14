//! The driver: the socket and the clock the sans-IO agent does without ([spec] §2, §15).
//!
//! The division is the whole point of the shape. [`Agent`](super::Agent) owns the protocol —
//! which check goes out next, when it is retransmitted, which pair wins — and this task owns the
//! two things a state machine must not: a `UdpSocket` and a deadline. ICE protocol timers are
//! still only the [`Output::SetTimer`] values the sans-I/O agent hands it. TURN allocation,
//! permission and retransmission deadlines are a separate socket-owner lifecycle from RFC 8656;
//! they are bounded here and never manufactured as ICE inputs.
//!
//! That last rule is not tidiness. A driver with a timer of its own can keep an agent that has
//! stopped asking for ticks alive, which makes a dead pacing path look healthy from the outside —
//! the exact defect `M-21`'s review found in a *test* that fired Ta by hand, one layer up. The
//! ICE deadline table here holds only what the agent put in it, a fired one-shot is removed before
//! the agent sees it, and nothing re-arms it but the agent's own next output.
//!
//! The other rule is subtractive: the driver feeds the agent only inputs [spec] §2 names, and only
//! when the thing they describe actually happened. Manufacturing an input — replaying a datagram,
//! synthesising a `DataSent` for media that did not go out — is how an outside caller reintroduces
//! the triggered-check storm `da9d49f` fixed on the inside.
//!
//! [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::RwLock as StdRwLock;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::sync::{Mutex, mpsc, oneshot, watch};

use sipx_sdp::ice::{Candidate, CandidateType, ComponentId, Credentials, RemoteCandidate};

use super::agent::{Agent, Input, Output, Timer};
use super::candidate::LocalBase;
use super::turn::{self, Allocation};
use crate::counters::DiscardMeters;

/// How many events may queue for the driver before the media path stops offering them.
///
/// Small on purpose. Everything on this channel is either a datagram that has already been read
/// off the socket or a note that a media packet went out, and none of it is worth blocking a
/// receive loop or a send loop for: a driver that has fallen this far behind will not catch up by
/// being given more.
const EVENTS: usize = 64;
/// Two component allocations times the ICE pair-set ceiling, plus their Refresh operations.
/// A hostile peer cannot make the outstanding TURN transaction set larger than this.
const TURN_TRANSACTIONS: usize = 202;
/// The ICE checklist is capped at 100 pairs, so no more first checks need wait on permissions.
const TURN_DEFERRED: usize = 100;
/// RFC 8489 §7.2.1's initial UDP retransmission timeout.
const TURN_RTO: Duration = Duration::from_millis(500);
/// Six retransmits after the initial send gives the RFC's seven-request bounded ladder.
const TURN_RETRANSMITS: u8 = 6;

/// The candidate path an ICE-backed media session actually selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum IcePath {
    /// The session did not negotiate ICE.
    Disabled,
    /// ICE is running but has not selected an RTP pair yet.
    Checking,
    /// Both ends of the selected pair are host candidates.
    Host,
    /// At least one end of the selected pair is server-reflexive.
    ServerReflexive,
    /// At least one end is peer-reflexive, and neither is relayed or server-reflexive.
    PeerReflexive,
    /// At least one end of the selected pair is relayed.
    Relayed,
}

impl IcePath {
    const fn encoded(self) -> u8 {
        match self {
            Self::Disabled => 0,
            Self::Checking => 1,
            Self::Host => 2,
            Self::ServerReflexive => 3,
            Self::PeerReflexive => 4,
            Self::Relayed => 5,
        }
    }

    fn decoded(encoded: u8) -> Self {
        match encoded {
            2 => Self::Host,
            3 => Self::ServerReflexive,
            4 => Self::PeerReflexive,
            5 => Self::Relayed,
            _ => Self::Checking,
        }
    }

    fn selected(local: CandidateType, remote: CandidateType) -> Self {
        if matches!(local, CandidateType::Relayed) || matches!(remote, CandidateType::Relayed) {
            Self::Relayed
        } else if matches!(local, CandidateType::ServerReflexive)
            || matches!(remote, CandidateType::ServerReflexive)
        {
            Self::ServerReflexive
        } else if matches!(local, CandidateType::PeerReflexive)
            || matches!(remote, CandidateType::PeerReflexive)
        {
            Self::PeerReflexive
        } else {
            Self::Host
        }
    }
}

/// What the media path tells the driver about.
///
/// Exactly two things, and both are facts rather than requests. There is no "send a check now" —
/// that decision is the agent's, and a channel that could carry it would be a second scheduler.
#[derive(Debug)]
pub(crate) enum Event {
    /// A datagram [`crate::dtls::classify`] called STUN (RFC 5764 §5.1.2), and where it came from.
    Datagram {
        /// Its source address.
        from: SocketAddr,
        /// Which of our sockets it arrived on.
        on: LocalBase,
        /// Direct UDP or an embedded datagram unwrapped from a TURN Data indication.
        via: CandidateType,
        /// The bytes, exactly as they arrived.
        bytes: Vec<u8>,
    },
    /// Media went out on a component's selected pair, which resets that pair's keepalive (§11).
    DataSent {
        /// Which component carried it.
        component: ComponentId,
    },
    /// A later offer or answer on this call carried the peer's ICE half (RFC 8839 §4.4; [spec]
    /// §13.5).
    ///
    /// This is the third fact, and it is a fact like the other two: a description arrived. What it
    /// means — merge the candidates, or rebuild for a restart — is the agent's to decide, exactly
    /// as it decides for the description that started the session.
    ///
    /// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
    Renegotiated {
        /// The local parameters to adopt first, when this exchange is a restart this side is
        /// offering or answering. `None` leaves the running session's credentials in place.
        local: Option<(Credentials, u64)>,
        /// The peer's half, when the description carried one. `None` is a restart this side is
        /// offering, whose answer has not arrived yet.
        peer: Option<Peer>,
        /// Where to send back what the next description must signal, once both are applied.
        reply: oneshot::Sender<Local>,
    },
}

/// The peer's ICE half, as [`super::Negotiation`] read it out of a description.
#[derive(Debug)]
pub(crate) struct Peer {
    pub(crate) credentials: Credentials,
    pub(crate) candidates: Vec<Candidate>,
    pub(crate) lite: bool,
}

/// What this side must put in its next offer or answer for the stream ([spec] §13.5).
///
/// Non-exhaustive: this is the list of ICE attributes a description must carry, and RFC 8839 has
/// more of them than the two that matter today — `a=ice-options` and `a=ice-lite` are attributes
/// the agent knows about itself and does not yet report here.
///
/// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Local {
    /// `a=ice-ufrag` and `a=ice-pwd`, read back from the agent rather than from the caller's copy.
    pub credentials: Credentials,
    /// `a=candidate`, priced by the agent, in descending priority.
    pub candidates: Vec<Candidate>,
    /// `a=remote-candidates`, present only for a controlling, Completed current generation.
    pub remote_candidates: Vec<RemoteCandidate>,
}

impl Local {
    /// What one description must signal for this stream.
    ///
    /// Both arguments: a half with no credentials cannot be checked against and a half with no
    /// candidates offers nowhere to check, so neither has a default that describes a usable
    /// stream. An ICE restart legitimately signals new credentials with candidates still
    /// gathering, and that case reaches a caller as an empty vector it passed in rather than as a
    /// field this constructor left out.
    #[must_use]
    pub const fn new(credentials: Credentials, candidates: Vec<Candidate>) -> Self {
        Self {
            credentials,
            candidates,
            remote_candidates: Vec::new(),
        }
    }
}

/// The handle the media path holds: where to send events, and whether it is worth sending them.
#[derive(Debug, Clone)]
pub(crate) struct Handle {
    events: mpsc::Sender<Event>,
    discards: Arc<DiscardMeters>,
    /// Whether a pair has been selected for component 1.
    ///
    /// Read by the send loop before it reports a packet, so that the fifty notes a second an
    /// ordinary call would produce are not even constructed until there is a selected pair for
    /// them to be about. §11's keepalive is only ever on a selected pair, so before there is one
    /// the agent would discard every one of them.
    selected: Arc<AtomicBool>,
    path: Arc<AtomicU8>,
    relay_routes: Arc<StdRwLock<Vec<RelayRoute>>>,
    relay_servers: Arc<Vec<(LocalBase, SocketAddr)>>,
    #[cfg_attr(not(feature = "dtls"), allow(dead_code))]
    selection: watch::Receiver<Selection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "dtls"), allow(dead_code))]
enum Selection {
    Checking,
    Selected(crate::browser::SelectedComponent),
    Failed,
}

/// Why component 1 produced no nominated pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[cfg(feature = "dtls")]
pub(crate) enum SelectionError {
    /// ICE exhausted component 1 without a nominated pair.
    #[error("ICE failed before nominating component 1")]
    Failed,
    /// The driver stopped before reporting either selection or failure.
    #[error("ICE stopped before nominating component 1")]
    Stopped,
}

impl Handle {
    /// The RTP candidate path selected so far.
    pub(crate) fn path(&self) -> IcePath {
        IcePath::decoded(self.path.load(Ordering::Relaxed))
    }

    /// Wait for component 1's exact selected pair or its terminal failure.
    #[cfg(feature = "dtls")]
    pub(crate) async fn wait_selected(
        &self,
        ice_generation: u64,
    ) -> Result<crate::browser::SelectedComponent, SelectionError> {
        let mut selection = self.selection.clone();
        loop {
            match *selection.borrow_and_update() {
                Selection::Selected(mut selected) => {
                    selected.ice_generation = ice_generation;
                    return Ok(selected);
                }
                Selection::Failed => return Err(SelectionError::Failed),
                Selection::Checking => {}
            }
            selection
                .changed()
                .await
                .map_err(|_| SelectionError::Stopped)?;
        }
    }
    /// Hand the driver a datagram. Non-blocking: a full queue drops it.
    ///
    /// Dropping is right and not merely convenient. A connectivity check is a retransmitted
    /// transaction (RFC 5389 §7.2.1) and the far end will send it again; blocking the receive loop
    /// on a slow driver would stall the *audio* to protect a check that is already redundant.
    pub(crate) fn datagram(
        &self,
        from: SocketAddr,
        on: LocalBase,
        via: CandidateType,
        bytes: Vec<u8>,
    ) -> bool {
        if self
            .events
            .try_send(Event::Datagram {
                from,
                on,
                via,
                bytes,
            })
            .is_err()
        {
            self.discards
                .ice_driver_queue_refusals
                .fetch_add(1, Ordering::Relaxed);
            tracing::debug!(%from, "dropping a connectivity check the ice driver could not take");
            false
        } else {
            true
        }
    }

    /// Wrap a media datagram in a TURN Send indication when this component selected a relayed
    /// local candidate. `None` means the ordinary direct send path remains in force.
    pub(crate) fn relay_datagram(
        &self,
        component: ComponentId,
        peer: SocketAddr,
        bytes: &[u8],
    ) -> Option<(SocketAddr, Vec<u8>)> {
        let routes = self.relay_routes.read().ok()?;
        let route = routes.iter().find(|route| route.component == component)?;
        let datagram = turn::send_indication(turn::new_transaction_id(), peer, bytes).ok()?;
        Some((route.server, datagram))
    }

    /// Unwrap a Data indication only when it came from the allocation server for this base.
    pub(crate) fn relayed_data<'a>(
        &self,
        from: SocketAddr,
        on: LocalBase,
        bytes: &'a [u8],
    ) -> Option<turn::PeerData<'a>> {
        self.relay_servers
            .iter()
            .any(|(base, server)| *base == on && *server == from)
            .then(|| turn::data_indication(bytes).ok())
            .flatten()
    }

    /// Apply a later exchange's ICE half and read back what the next description must signal.
    ///
    /// Awaited rather than dropped on a full queue, which is the opposite of
    /// [`Self::datagram`]'s rule and for the opposite reason: a connectivity check is
    /// retransmitted by the far end, and an offer/answer is not. Losing one silently would leave
    /// the agent keyed to credentials the peer has stopped using, so the checks would authenticate
    /// against nothing and the caller would signal candidates for a session that no longer exists.
    ///
    /// `None` when the driver has stopped — the session is ending, and the caller answers without
    /// ICE attributes rather than waiting for a task that will never reply.
    pub(crate) async fn renegotiated(
        &self,
        local: Option<(Credentials, u64)>,
        peer: Option<Peer>,
    ) -> Option<Local> {
        let (reply, answered) = oneshot::channel();
        self.events
            .send(Event::Renegotiated { local, peer, reply })
            .await
            .ok()?;
        answered.await.ok()
    }

    /// Note that media went out, if there is a selected pair for it to have gone out on.
    pub(crate) fn data_sent(&self, component: ComponentId) {
        if !self.selected.load(Ordering::Relaxed) {
            return;
        }
        // A dropped note costs one keepalive that did not need to be sent; §11's indication is
        // unauthenticated and draws no response, so it is the cheapest thing here to lose.
        if self.events.try_send(Event::DataSent { component }).is_err() {
            self.discards
                .ice_data_sent_queue_refusals
                .fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Where the media path sends, and what the driver moves when ICE concludes.
///
/// Shared with the send loop and the report loop rather than pushed to them, because those loops
/// already read `remote` on every packet: making the selected pair a write to the same cell is
/// what makes the switch atomic with respect to a packet in flight.
#[derive(Debug, Clone)]
pub(crate) struct Destinations {
    /// Component 1: where RTP goes. Starts at the `c=`/`m=` default destination.
    pub(crate) rtp: Arc<Mutex<SocketAddr>>,
    /// Component 2: where RTCP goes, once ICE has selected a pair for it.
    ///
    /// `None` leaves the report loop on RFC 3550 §11's convention — the RTP destination's port
    /// plus one — which is what it does for a stream with no ICE and what it must keep doing for
    /// a stream whose second component never concluded.
    pub(crate) rtcp: Arc<Mutex<Option<SocketAddr>>>,
}

/// Bound component sockets and the TURN allocations whose five-tuples they own.
pub(crate) struct BoundSockets {
    sockets: Vec<Arc<UdpSocket>>,
    allocations: Vec<Allocation>,
}

impl BoundSockets {
    pub(crate) fn new(sockets: Vec<Arc<UdpSocket>>, allocations: Vec<Allocation>) -> Self {
        Self {
            sockets,
            allocations,
        }
    }
}

/// The running driver.
struct Driver {
    agent: Agent,
    /// The sockets, indexed by the [`LocalBase`] the agent names them with ([spec] §2).
    ///
    /// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
    sockets: Vec<Arc<UdpSocket>>,
    /// What the agent has asked to be woken for, and when. **Only** what the agent asked for.
    deadlines: HashMap<Timer, tokio::time::Instant>,
    events: mpsc::Receiver<Event>,
    destinations: Destinations,
    selected: Arc<AtomicBool>,
    path: Arc<AtomicU8>,
    selection: watch::Sender<Selection>,
    allocations: Vec<Allocation>,
    allocation_refresh: Vec<tokio::time::Instant>,
    permission_refresh: Option<tokio::time::Instant>,
    turn_pending: Vec<TurnPending>,
    permissions: HashSet<(usize, IpAddr)>,
    deferred_relay: Vec<DeferredRelay>,
    relay_routes: Arc<StdRwLock<Vec<RelayRoute>>>,
    stop: Arc<crate::session::Stop>,
    discards: Arc<DiscardMeters>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RelayRoute {
    component: ComponentId,
    server: SocketAddr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TurnOperation {
    Refresh,
    Permission(IpAddr),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TurnPending {
    base: LocalBase,
    transaction: turn::TransactionId,
    allocation: usize,
    operation: TurnOperation,
    bytes: Vec<u8>,
    retry_at: tokio::time::Instant,
    rto: Duration,
    retransmits: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeferredRelay {
    base: LocalBase,
    peer: SocketAddr,
    bytes: Vec<u8>,
}

/// A browser component retains both halves so shutdown can join the ICE task.
#[cfg(feature = "dtls")]
pub(crate) struct OwnedDriver {
    pub(crate) handle: Handle,
    pub(crate) task: tokio::task::JoinHandle<()>,
}

/// Start the driver for a stream, and hand the media path its end of it.
pub(crate) fn spawn(
    agent: Agent,
    pending: Vec<Output>,
    bound: BoundSockets,
    destinations: Destinations,
    stop: Arc<crate::session::Stop>,
    discards: Arc<DiscardMeters>,
) -> (Handle, tokio::task::JoinHandle<()>) {
    spawn_parts(agent, pending, bound, destinations, stop, discards, None)
}

#[cfg(feature = "dtls")]
pub(crate) fn spawn_owned(
    agent: Agent,
    pending: Vec<Output>,
    bound: BoundSockets,
    destinations: Destinations,
    stop: Arc<crate::session::Stop>,
    discards: Arc<DiscardMeters>,
    profile_tasks: Arc<crate::browser::ProfileTasks>,
) -> OwnedDriver {
    let (handle, task) = spawn_parts(
        agent,
        pending,
        bound,
        destinations,
        stop,
        discards,
        Some(profile_tasks),
    );
    OwnedDriver { handle, task }
}

fn spawn_parts(
    agent: Agent,
    pending: Vec<Output>,
    bound: BoundSockets,
    destinations: Destinations,
    stop: Arc<crate::session::Stop>,
    discards: Arc<DiscardMeters>,
    #[cfg_attr(not(feature = "dtls"), allow(unused_variables))] profile_tasks: Option<
        Arc<crate::browser::ProfileTasks>,
    >,
) -> (Handle, tokio::task::JoinHandle<()>) {
    let BoundSockets {
        sockets,
        allocations,
    } = bound;
    let (events_tx, events_rx) = mpsc::channel(EVENTS);
    let selected = Arc::new(AtomicBool::new(false));
    let path = Arc::new(AtomicU8::new(IcePath::Checking.encoded()));
    let (selection, selected_pair) = watch::channel(Selection::Checking);
    let relay_routes = Arc::new(StdRwLock::new(Vec::new()));
    let relay_servers = Arc::new(
        allocations
            .iter()
            .map(|allocation| (allocation.base, allocation.server))
            .collect(),
    );
    let driver = Driver {
        agent,
        sockets,
        deadlines: HashMap::new(),
        events: events_rx,
        destinations,
        selected: Arc::clone(&selected),
        path: Arc::clone(&path),
        selection,
        allocations,
        allocation_refresh: Vec::new(),
        permission_refresh: None,
        turn_pending: Vec::new(),
        permissions: HashSet::new(),
        deferred_relay: Vec::new(),
        relay_routes: Arc::clone(&relay_routes),
        stop,
        discards: Arc::clone(&discards),
    };
    let task = if let Some(profile_tasks) = profile_tasks {
        tokio::spawn(crate::browser::profile_task(
            profile_tasks,
            driver.run(pending),
        ))
    } else {
        tokio::spawn(driver.run(pending))
    };
    let handle = Handle {
        events: events_tx,
        selected,
        path,
        selection: selected_pair,
        relay_routes,
        relay_servers,
        discards,
    };
    (handle, task)
}

impl Driver {
    /// The loop. One `select!` over three things: the stop signal, an event from the media path,
    /// and the earliest deadline the agent has asked for.
    async fn run(mut self, pending: Vec<Output>) {
        self.start_turn().await;
        self.apply(pending).await;

        loop {
            if self.stop.is_stopped() {
                break;
            }
            // Recomputed every pass, because the agent may have moved, cleared or added a
            // deadline while handling the last event. There is no timer here that outlives the
            // pass that armed it.
            let next = self
                .deadlines
                .iter()
                .min_by_key(|(_, at)| **at)
                .map(|(timer, at)| (*timer, *at));
            let next_turn = self.next_turn_deadline();

            // Disabled outright when the agent has asked for nothing, so an agent that has gone
            // quiet is not woken by a deadline this loop invented. The instant in that case is
            // never waited on — the guard is what makes the arm inert.
            let deadline = match (next.map(|(_, at)| at), next_turn) {
                (Some(agent), Some(turn)) => agent.min(turn),
                (Some(agent), None) => agent,
                (None, Some(turn)) => turn,
                (None, None) => tokio::time::Instant::now(),
            };
            let event = tokio::select! {
                () = self.stop.wait() => break,
                event = self.events.recv() => event,
                () = tokio::time::sleep_until(deadline), if next.is_some() || next_turn.is_some() => {
                    let now = tokio::time::Instant::now();
                    if let Some((timer, at)) = next
                        && at <= now
                    {
                        // A one-shot that has fired is no longer armed. Removing it *before* the
                        // agent sees it is what makes the next one the agent's to ask for.
                        self.deadlines.remove(&timer);
                        let outputs = self.agent.handle(Input::TimerFired(timer));
                        self.apply(outputs).await;
                    }
                    self.service_turn(now).await;
                    continue;
                }
            };

            let Some(event) = event else {
                // Every sender is gone, which means the session's loops have ended.
                break;
            };
            let outputs = match event {
                Event::Datagram {
                    from,
                    on,
                    via,
                    bytes,
                } => {
                    if self.turn_response(on, from, &bytes).await {
                        Vec::new()
                    } else {
                        self.agent.handle(Input::Datagram {
                            from,
                            on,
                            via,
                            bytes,
                        })
                    }
                }
                Event::DataSent { component } => self.agent.handle(Input::DataSent { component }),
                Event::Renegotiated { local, peer, reply } => {
                    let outputs = self.renegotiated(local, peer);
                    // Dropped receiver means the signalling side gave up on this exchange; the
                    // agent has still applied it, which is correct — the peer's credentials
                    // changed whether or not anybody is waiting to hear what ours are.
                    if reply
                        .send(Local {
                            credentials: self.agent.credentials().clone(),
                            candidates: super::gather::lines(self.agent.local_candidates()),
                            remote_candidates: self.agent.selected_remote_candidates(),
                        })
                        .is_err()
                    {
                        self.discards
                            .ice_renegotiation_reply_unobserved
                            .fetch_add(1, Ordering::Relaxed);
                    }
                    outputs
                }
            };
            self.apply(outputs).await;
        }
        self.delete_allocations().await;
    }

    async fn start_turn(&mut self) {
        if self.allocations.is_empty() {
            return;
        }
        self.refresh_permissions().await;
        let now = tokio::time::Instant::now();
        self.allocation_refresh = self
            .allocations
            .iter()
            .map(|allocation| now + half_lifetime(allocation.lifetime))
            .collect();
        self.permission_refresh = Some(now + Duration::from_secs(240));
    }

    fn next_turn_deadline(&self) -> Option<tokio::time::Instant> {
        self.allocation_refresh
            .iter()
            .copied()
            .chain(self.permission_refresh)
            .chain(self.turn_pending.iter().map(|pending| pending.retry_at))
            .min()
    }

    async fn service_turn(&mut self, now: tokio::time::Instant) {
        self.retransmit_turn(now).await;
        let due: Vec<usize> = self
            .allocation_refresh
            .iter()
            .enumerate()
            .filter_map(|(index, at)| (*at <= now).then_some(index))
            .collect();
        for index in due {
            self.send_refresh(index).await;
            let Some(allocation) = self.allocations.get(index) else {
                continue;
            };
            if let Some(deadline) = self.allocation_refresh.get_mut(index) {
                *deadline = now + half_lifetime(allocation.lifetime);
            }
        }
        if self.permission_refresh.is_some_and(|at| at <= now) {
            self.refresh_permissions().await;
            self.permission_refresh = Some(now + Duration::from_secs(240));
        }
    }

    async fn retransmit_turn(&mut self, now: tokio::time::Instant) {
        let due: Vec<(LocalBase, turn::TransactionId)> = self
            .turn_pending
            .iter()
            .filter(|pending| pending.retry_at <= now)
            .map(|pending| (pending.base, pending.transaction))
            .collect();
        for (base, transaction) in due {
            let Some(position) = self
                .turn_pending
                .iter()
                .position(|pending| pending.base == base && pending.transaction == transaction)
            else {
                continue;
            };
            let Some((retransmits, allocation, bytes)) =
                self.turn_pending.get(position).map(|pending| {
                    (
                        pending.retransmits,
                        pending.allocation,
                        pending.bytes.clone(),
                    )
                })
            else {
                continue;
            };
            if retransmits == 0 {
                self.turn_pending.remove(position);
                continue;
            }
            let Some(server) = self.allocations.get(allocation).map(|value| value.server) else {
                self.turn_pending.remove(position);
                continue;
            };
            let Some(socket) = self.sockets.get(usize::from(base.0)).cloned() else {
                self.turn_pending.remove(position);
                continue;
            };
            if let Err(error) = socket.send_to(&bytes, server).await {
                self.discards
                    .ice_send_failures
                    .fetch_add(1, Ordering::Relaxed);
                tracing::debug!(%server, %error, "a TURN retransmission could not be sent");
            }
            if let Some(pending) = self.turn_pending.get_mut(position) {
                pending.rto = pending.rto.saturating_mul(2);
                pending.retry_at = now + pending.rto;
                pending.retransmits = pending.retransmits.saturating_sub(1);
            }
        }
    }

    async fn refresh_permissions(&mut self) {
        let mut peers: Vec<IpAddr> = self
            .agent
            .remote_candidates()
            .iter()
            .map(|candidate| candidate.address.ip())
            .collect();
        peers.sort_unstable();
        peers.dedup();
        let jobs: Vec<(usize, IpAddr)> = self
            .allocations
            .iter()
            .enumerate()
            .flat_map(|(index, allocation)| {
                peers.iter().copied().filter_map(move |peer| {
                    matches!(
                        (peer, allocation.relayed.ip()),
                        (IpAddr::V4(_), IpAddr::V4(_)) | (IpAddr::V6(_), IpAddr::V6(_))
                    )
                    .then_some((index, peer))
                })
            })
            .collect();
        for (index, peer) in jobs.into_iter().take(TURN_TRANSACTIONS) {
            self.send_permission(index, peer).await;
        }
    }

    async fn send_refresh(&mut self, index: usize) {
        let Some(allocation) = self.allocations.get(index) else {
            return;
        };
        let (base, server, lifetime, auth) = (
            allocation.base,
            allocation.server,
            allocation.lifetime,
            allocation.auth.clone(),
        );
        let Some(socket) = self.sockets.get(usize::from(base.0)).cloned() else {
            return;
        };
        let transaction = turn::new_transaction_id();
        let Ok(bytes) = turn::refresh(transaction, lifetime, &auth) else {
            return;
        };
        let pending = TurnPending {
            base,
            transaction,
            allocation: index,
            operation: TurnOperation::Refresh,
            bytes: bytes.clone(),
            retry_at: tokio::time::Instant::now() + TURN_RTO,
            rto: TURN_RTO,
            retransmits: TURN_RETRANSMITS,
        };
        if self.track_turn(pending)
            && let Err(error) = socket.send_to(&bytes, server).await
        {
            self.discards
                .ice_send_failures
                .fetch_add(1, Ordering::Relaxed);
            tracing::debug!(%server, %error, "a TURN refresh could not be sent");
        }
    }

    async fn send_permission(&mut self, index: usize, peer: IpAddr) {
        let Some(allocation) = self.allocations.get(index) else {
            return;
        };
        let (base, server, auth) = (allocation.base, allocation.server, allocation.auth.clone());
        let Some(socket) = self.sockets.get(usize::from(base.0)).cloned() else {
            return;
        };
        let transaction = turn::new_transaction_id();
        let Ok(bytes) = turn::create_permission(transaction, SocketAddr::new(peer, 9), &auth)
        else {
            return;
        };
        let pending = TurnPending {
            base,
            transaction,
            allocation: index,
            operation: TurnOperation::Permission(peer),
            bytes: bytes.clone(),
            retry_at: tokio::time::Instant::now() + TURN_RTO,
            rto: TURN_RTO,
            retransmits: TURN_RETRANSMITS,
        };
        if self.track_turn(pending)
            && let Err(error) = socket.send_to(&bytes, server).await
        {
            self.discards
                .ice_send_failures
                .fetch_add(1, Ordering::Relaxed);
            tracing::debug!(%server, %error, "a TURN permission request could not be sent");
        }
    }

    fn track_turn(&mut self, pending: TurnPending) -> bool {
        self.turn_pending
            .retain(|known| known.base != pending.base || known.operation != pending.operation);
        if self.turn_pending.len() < TURN_TRANSACTIONS {
            self.turn_pending.push(pending);
            true
        } else {
            // discard: the fixed outstanding-transaction budget is deliberate backpressure;
            // the periodic permission/refresh schedule will retry operations that remain needed.
            tracing::warn!("TURN transaction bound reached; refusing another operation");
            false
        }
    }

    async fn turn_response(&mut self, on: LocalBase, from: SocketAddr, bytes: &[u8]) -> bool {
        if !self
            .allocations
            .iter()
            .any(|allocation| allocation.base == on && allocation.server == from)
        {
            return false;
        }
        let Ok(response) = turn::response(bytes) else {
            return false;
        };
        let Some(position) = self.turn_pending.iter().position(|pending| {
            pending.base == on && pending.transaction == response.transaction()
        }) else {
            // It is a TURN response from this base's configured server, but not to any live
            // operation. Drop it here rather than handing a non-Binding message to the ICE agent.
            return true;
        };
        let Some(pending) = self.turn_pending.get(position).cloned() else {
            return true;
        };
        let Some(allocation) = self.allocations.get(pending.allocation) else {
            return true;
        };
        let operation_matches = match pending.operation {
            TurnOperation::Refresh => response.is_refresh(),
            TurnOperation::Permission(_) => response.is_permission(),
        };
        if allocation.base != on
            || allocation.server != from
            || !response.verify(&allocation.auth)
            || !operation_matches
        {
            return true;
        }
        self.turn_pending.remove(position);
        match response {
            turn::Response::Challenge {
                code: 438,
                realm: Some(realm),
                nonce: Some(nonce),
                password_algorithms,
                ..
            } => {
                if let Some(allocation) = self.allocations.get_mut(pending.allocation)
                    && allocation
                        .auth
                        .replace_challenge(&realm, nonce, password_algorithms)
                        .is_err()
                {
                    return true;
                }
                match pending.operation {
                    TurnOperation::Refresh => self.send_refresh(pending.allocation).await,
                    TurnOperation::Permission(peer) => {
                        self.send_permission(pending.allocation, peer).await;
                    }
                }
            }
            turn::Response::Success {
                lifetime: Some(lifetime),
                ..
            } if pending.operation == TurnOperation::Refresh => {
                if let Some(allocation) = self.allocations.get_mut(pending.allocation) {
                    allocation.lifetime = lifetime;
                }
                if let Some(deadline) = self.allocation_refresh.get_mut(pending.allocation) {
                    *deadline = tokio::time::Instant::now() + half_lifetime(lifetime);
                }
            }
            turn::Response::Success { .. } => {
                if let TurnOperation::Permission(peer) = pending.operation {
                    self.permissions.insert((pending.allocation, peer));
                    self.flush_permission(pending.allocation, peer).await;
                }
            }
            _ => {}
        }
        true
    }

    async fn send_relayed(&mut self, base: LocalBase, peer: SocketAddr, bytes: Vec<u8>) {
        let Some(allocation) = self
            .allocations
            .iter()
            .position(|allocation| allocation.base == base)
        else {
            return;
        };
        if self.permissions.contains(&(allocation, peer.ip())) {
            self.send_relay_now(allocation, peer, &bytes).await;
            return;
        }
        let deferred = DeferredRelay { base, peer, bytes };
        if self.deferred_relay.len() < TURN_DEFERRED && !self.deferred_relay.contains(&deferred) {
            self.deferred_relay.push(deferred);
        }
        let already_pending = self.turn_pending.iter().any(|pending| {
            pending.allocation == allocation
                && pending.operation == TurnOperation::Permission(peer.ip())
        });
        if !already_pending {
            self.send_permission(allocation, peer.ip()).await;
        }
    }

    async fn flush_permission(&mut self, allocation: usize, peer: IpAddr) {
        let deferred = std::mem::take(&mut self.deferred_relay);
        for datagram in deferred {
            let belongs = self
                .allocations
                .get(allocation)
                .is_some_and(|known| known.base == datagram.base)
                && datagram.peer.ip() == peer;
            if belongs {
                self.send_relay_now(allocation, datagram.peer, &datagram.bytes)
                    .await;
            } else {
                self.deferred_relay.push(datagram);
            }
        }
    }

    async fn send_relay_now(&self, allocation: usize, peer: SocketAddr, bytes: &[u8]) {
        let Some(allocation) = self.allocations.get(allocation) else {
            return;
        };
        let Some(socket) = self.sockets.get(usize::from(allocation.base.0)) else {
            return;
        };
        let Ok(indication) = turn::send_indication(turn::new_transaction_id(), peer, bytes) else {
            return;
        };
        if let Err(error) = socket.send_to(&indication, allocation.server).await {
            self.discards
                .ice_send_failures
                .fetch_add(1, Ordering::Relaxed);
            tracing::debug!(%peer, %error, "a relayed datagram could not be sent");
        }
    }

    async fn delete_allocations(&self) {
        for allocation in &self.allocations {
            let Some(socket) = self.sockets.get(usize::from(allocation.base.0)) else {
                continue;
            };
            if let Ok(bytes) =
                turn::refresh(turn::new_transaction_id(), Duration::ZERO, &allocation.auth)
                && let Err(error) = socket.send_to(&bytes, allocation.server).await
            {
                self.discards
                    .ice_send_failures
                    .fetch_add(1, Ordering::Relaxed);
                tracing::debug!(
                    server = %allocation.server,
                    %error,
                    "a TURN allocation deletion could not be sent"
                );
            }
        }
    }

    /// Apply a later exchange's ICE half to the running agent (RFC 8839 §4.4; [spec] §13.5).
    ///
    /// The order is the contract and not an implementation detail. Our own parameters go in
    /// **first**, so that when the peer's description turns out to be a restart, the checklists the
    /// agent rebuilds are keyed to the credentials this side is about to signal rather than to the
    /// ones the finished session used. Applied the other way round, the new session would start
    /// authenticating with credentials the peer has already been told to forget.
    ///
    /// Whether this *is* a restart is not decided here. It is RFC 8839 §4.4.1.1.1's question about
    /// the peer's two credentials, the agent has always answered it, and asking it a second time
    /// here would be a second place for the answer to drift.
    ///
    /// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
    fn renegotiated(
        &mut self,
        local: Option<(Credentials, u64)>,
        peer: Option<Peer>,
    ) -> Vec<Output> {
        let mut outputs = Vec::new();
        if let Some((credentials, tiebreaker)) = local {
            outputs.extend(self.agent.handle(Input::LocalCredentials {
                credentials,
                tiebreaker,
            }));
        }
        if let Some(peer) = peer {
            outputs.extend(self.agent.handle(Input::RemoteDescription {
                credentials: peer.credentials,
                candidates: peer.candidates,
                lite: peer.lite,
            }));
        }
        outputs
    }

    /// Perform the agent's outputs, **in the order given** ([spec] §2): a `Send` always precedes
    /// the `SetTimer` that would retransmit it, so this loop is sequential and awaits each send
    /// before it arms anything.
    ///
    /// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
    async fn apply(&mut self, outputs: Vec<Output>) {
        for output in outputs {
            match output {
                Output::Send {
                    on,
                    kind,
                    to,
                    bytes,
                } => {
                    if kind == CandidateType::Relayed {
                        self.send_relayed(on, to, bytes).await;
                        continue;
                    }
                    let Some(socket) = self.sockets.get(usize::from(on.0)) else {
                        // The agent named a base the driver did not bind. It cannot: every base
                        // it knows came from a `LocalCandidate` this driver gathered.
                        // discard: every base the agent can name came from a candidate gathered
                        // over this exact socket vector, so this branch is structurally unreachable.
                        tracing::warn!(base = on.0, "no socket for the base the agent named");
                        continue;
                    };
                    if let Err(error) = socket.send_to(&bytes, to).await {
                        // One unreachable candidate is an ordinary thing to find — it is what
                        // checking is for — and the pair fails on its own timer rather than here.
                        self.discards
                            .ice_send_failures
                            .fetch_add(1, Ordering::Relaxed);
                        tracing::debug!(%to, %error, "a connectivity check could not be sent");
                    }
                }
                Output::SetTimer { timer, after } => {
                    let at = tokio::time::Instant::now()
                        .checked_add(after)
                        .unwrap_or_else(tokio::time::Instant::now);
                    self.deadlines.insert(timer, at);
                }
                Output::ClearTimer(timer) => {
                    self.deadlines.remove(&timer);
                }
                Output::Selected {
                    component,
                    local,
                    local_kind,
                    remote,
                    remote_kind,
                } => {
                    self.select(component, local, local_kind, remote, remote_kind)
                        .await;
                }
                Output::Failed { component } => {
                    // The call layer decides what a failed component means ([spec] §2). What the
                    // media path does is nothing: the stream keeps sending to the default
                    // destination, which is where it was already sending.
                    // discard: this is the agent's terminal outcome, not a payload with a later
                    // consumer; the default path remains active.
                    tracing::warn!(component = component.get(), "ice failed for a component");
                    if component == ComponentId::RTP {
                        self.selection.send_replace(Selection::Failed);
                    }
                }
            }
        }
    }

    /// Point the media at a selected pair (§8.1.1).
    ///
    /// This is the moment the stream stops being an SDP address and starts being a checked path,
    /// and it is also the moment symmetric RTP stops applying — the receive loop was told at
    /// startup not to learn, because on an ICE stream the address is ICE's to choose and an
    /// unauthenticated packet must not be able to move it.
    async fn select(
        &mut self,
        component: ComponentId,
        local: LocalBase,
        local_kind: CandidateType,
        remote: SocketAddr,
        remote_kind: CandidateType,
    ) {
        if component == ComponentId::RTP {
            if local != LocalBase(0) {
                // RTP leaves the media socket, which is base 0 by construction. A selected pair
                // on any other base would mean audio and its checks on different sockets, and
                // the far end would see media from an address it never validated.
                tracing::warn!(
                    base = local.0,
                    "a selected rtp pair on a base that is not the media socket"
                );
                return;
            }
            let Some(socket) = self.sockets.get(usize::from(local.0)) else {
                tracing::warn!(base = local.0, "selected pair names an unbound local base");
                return;
            };
            let Ok(local_address) = socket.local_addr() else {
                tracing::warn!(base = local.0, "selected pair's local base has no address");
                return;
            };
            *self.destinations.rtp.lock().await = remote;
            self.selected.store(true, Ordering::Relaxed);
            self.path.store(
                IcePath::selected(local_kind, remote_kind).encoded(),
                Ordering::Relaxed,
            );
            self.selection.send_replace(Selection::Selected(
                crate::browser::SelectedComponent::new(local_address, remote, 0)
                    .with_candidate_types(local_kind, remote_kind),
            ));
        } else {
            *self.destinations.rtcp.lock().await = Some(remote);
        }
        if local_kind == CandidateType::Relayed
            && let Some(allocation) = self
                .allocations
                .iter()
                .find(|allocation| allocation.base == local)
            && let Ok(mut routes) = self.relay_routes.write()
        {
            routes.retain(|route| route.component != component);
            routes.push(RelayRoute {
                component,
                server: allocation.server,
            });
        }
        tracing::debug!(component = component.get(), %remote, "ice selected a pair");
    }
}

fn half_lifetime(lifetime: Duration) -> Duration {
    lifetime.div_f64(2.0).max(Duration::from_secs(1))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::too_many_lines
)]
mod tests {
    use super::*;

    #[test]
    fn the_reported_path_is_derived_from_the_selected_pair_not_the_requested_policy() {
        assert_eq!(
            IcePath::selected(CandidateType::Host, CandidateType::Host),
            IcePath::Host
        );
        assert_eq!(
            IcePath::selected(CandidateType::Host, CandidateType::ServerReflexive),
            IcePath::ServerReflexive
        );
        assert_eq!(
            IcePath::selected(CandidateType::PeerReflexive, CandidateType::Host),
            IcePath::PeerReflexive
        );
        assert_eq!(
            IcePath::selected(CandidateType::Host, CandidateType::Relayed),
            IcePath::Relayed
        );
    }

    #[test]
    fn selected_relay_routes_wrap_send_and_unwrap_data_only_from_the_server() {
        let server: SocketAddr = "192.0.2.10:3478".parse().unwrap();
        let peer: SocketAddr = "203.0.113.9:40000".parse().unwrap();
        let (events, _receiver) = mpsc::channel(1);
        let (_selection, selected_pair) = watch::channel(Selection::Checking);
        let handle = Handle {
            events,
            discards: Arc::new(DiscardMeters::default()),
            selected: Arc::new(AtomicBool::new(true)),
            path: Arc::new(AtomicU8::new(IcePath::Relayed.encoded())),
            selection: selected_pair,
            relay_routes: Arc::new(StdRwLock::new(vec![RelayRoute {
                component: ComponentId::RTP,
                server,
            }])),
            relay_servers: Arc::new(vec![(LocalBase(0), server)]),
        };
        let payload = [0x80, 0x00, 0x00, 0x01];
        let (to, send) = handle
            .relay_datagram(ComponentId::RTP, peer, &payload)
            .expect("selected relay wraps media");
        assert_eq!(to, server);
        assert_eq!(
            turn::sent_data(&send).expect("Send indication"),
            turn::PeerData {
                peer,
                data: &payload,
            }
        );

        let data = turn::data_indication_for_test([7; 12], peer, &payload);
        assert_eq!(
            handle
                .relayed_data(server, LocalBase(0), &data)
                .expect("configured server unwraps Data"),
            turn::PeerData {
                peer,
                data: &payload,
            }
        );
        assert!(
            handle
                .relayed_data("192.0.2.11:3478".parse().unwrap(), LocalBase(0), &data)
                .is_none(),
            "a Data indication from another address is not an allocation packet"
        );
    }

    #[test]
    fn an_allocation_refreshes_before_its_granted_lifetime() {
        assert_eq!(
            half_lifetime(Duration::from_secs(600)),
            Duration::from_secs(300)
        );
        assert_eq!(
            half_lifetime(Duration::from_secs(1)),
            Duration::from_secs(1),
            "the refresh delay never collapses to a busy loop"
        );
    }

    #[tokio::test]
    async fn turn_responses_match_the_base_transaction_operation_and_integrity() {
        let server = UdpSocket::bind("127.0.0.1:0".parse::<SocketAddr>().unwrap())
            .await
            .expect("relay binds");
        let server_address = server.local_addr().unwrap();
        let first = Arc::new(
            UdpSocket::bind("127.0.0.1:0".parse::<SocketAddr>().unwrap())
                .await
                .expect("first base binds"),
        );
        let second = Arc::new(
            UdpSocket::bind("127.0.0.1:0".parse::<SocketAddr>().unwrap())
                .await
                .expect("second base binds"),
        );
        let relay = turn::Relay::new(server_address, "1000", "relay-password").unwrap();
        let auth = turn::Auth::challenged(&relay, "example.com", "nonce".to_owned(), None)
            .expect("legacy auth");
        let allocation = |base, relayed: &str| Allocation {
            base,
            server: server_address,
            relayed: relayed.parse().unwrap(),
            mapped: "198.51.100.20:50000".parse().unwrap(),
            lifetime: Duration::from_secs(600),
            auth: auth.clone(),
        };
        let (_events, event_rx) = mpsc::channel(1);
        let (selection, _selected) = watch::channel(Selection::Checking);
        let mut driver = Driver {
            agent: Agent::new(
                super::super::Config::default(),
                true,
                Credentials::new("aaaa", "asd88fgpdd777uzjYhagZg").unwrap(),
                7,
            ),
            sockets: vec![first, second],
            deadlines: HashMap::new(),
            events: event_rx,
            destinations: Destinations {
                rtp: Arc::new(Mutex::new("203.0.113.9:40000".parse().unwrap())),
                rtcp: Arc::new(Mutex::new(None)),
            },
            selected: Arc::new(AtomicBool::new(false)),
            path: Arc::new(AtomicU8::new(IcePath::Checking.encoded())),
            selection,
            allocations: vec![
                allocation(LocalBase(0), "192.0.2.44:49152"),
                allocation(LocalBase(1), "192.0.2.45:49153"),
            ],
            allocation_refresh: Vec::new(),
            permission_refresh: None,
            turn_pending: Vec::new(),
            permissions: HashSet::new(),
            deferred_relay: Vec::new(),
            relay_routes: Arc::new(StdRwLock::new(Vec::new())),
            stop: Arc::new(crate::session::Stop::default()),
            discards: Arc::new(DiscardMeters::default()),
        };
        let mut datagram = vec![0u8; 1500];

        driver.send_refresh(0).await;
        let (length, _) = server
            .recv_from(&mut datagram)
            .await
            .expect("first refresh");
        let first_id = turn::transaction_for_test(&datagram[..length]).expect("transaction");
        driver.send_refresh(1).await;
        let (length, _) = server
            .recv_from(&mut datagram)
            .await
            .expect("second refresh");
        let second_id = turn::transaction_for_test(&datagram[..length]).expect("transaction");

        let second_success =
            turn::refresh_success_for_test(second_id, Duration::from_secs(120), &auth);
        assert!(
            driver
                .turn_response(LocalBase(0), server_address, &second_success)
                .await
        );
        assert_eq!(driver.allocations[0].lifetime, Duration::from_secs(600));
        assert_eq!(driver.allocations[1].lifetime, Duration::from_secs(600));
        assert_eq!(driver.turn_pending.len(), 2, "wrong base consumes nothing");

        let wrong_operation = turn::authenticated_challenge_for_test(
            first_id,
            "permission",
            438,
            "wrong-operation",
            &auth,
        );
        assert!(
            driver
                .turn_response(LocalBase(0), server_address, &wrong_operation)
                .await
        );
        assert_eq!(
            driver.turn_pending.len(),
            2,
            "wrong operation consumes nothing"
        );

        let mut forged =
            turn::authenticated_challenge_for_test(first_id, "refresh", 438, "forged", &auth);
        *forged.last_mut().expect("integrity tag") ^= 1;
        assert!(
            driver
                .turn_response(LocalBase(0), server_address, &forged)
                .await
        );
        assert_eq!(
            driver.turn_pending.len(),
            2,
            "bad integrity consumes nothing"
        );

        let stale =
            turn::authenticated_challenge_for_test(first_id, "refresh", 438, "fresh", &auth);
        assert!(
            driver
                .turn_response(LocalBase(0), server_address, &stale)
                .await
        );
        let (length, _) = server
            .recv_from(&mut datagram)
            .await
            .expect("fresh-nonce refresh");
        let first_retry_id =
            turn::transaction_for_test(&datagram[..length]).expect("new transaction");
        assert_ne!(first_retry_id, first_id);

        assert!(
            driver
                .turn_response(LocalBase(1), server_address, &second_success)
                .await
        );
        assert_eq!(driver.allocations[1].lifetime, Duration::from_secs(120));
        assert_eq!(driver.allocations[0].lifetime, Duration::from_secs(600));

        let first_success =
            turn::refresh_success_for_test(first_retry_id, Duration::from_secs(90), &auth);
        assert!(
            driver
                .turn_response(LocalBase(0), server_address, &first_success)
                .await
        );
        assert_eq!(driver.allocations[0].lifetime, Duration::from_secs(90));
        assert!(driver.turn_pending.is_empty());

        let peer: SocketAddr = "203.0.113.9:40000".parse().unwrap();
        let payload = vec![0x00, 0x01, 0x02, 0x03];
        driver
            .send_relayed(LocalBase(0), peer, payload.clone())
            .await;
        let (length, _) = server
            .recv_from(&mut datagram)
            .await
            .expect("permission precedes the relayed check");
        assert_eq!(
            turn::operation_for_test(&datagram[..length]),
            Some("permission")
        );
        let permission_id = turn::transaction_for_test(&datagram[..length]).expect("transaction");
        for value in 0..=TURN_DEFERRED {
            driver
                .send_relayed(LocalBase(0), peer, vec![u8::try_from(value).unwrap_or(0)])
                .await;
        }
        assert_eq!(driver.deferred_relay.len(), TURN_DEFERRED);
        assert!(
            server.try_recv_from(&mut datagram).is_err(),
            "the Send indication waits for permission success"
        );

        let permission_success = turn::permission_success_for_test(permission_id, &auth);
        assert!(
            driver
                .turn_response(LocalBase(0), server_address, &permission_success)
                .await
        );
        let (length, _) = server
            .recv_from(&mut datagram)
            .await
            .expect("deferred Send indication");
        assert_eq!(
            turn::sent_data(&datagram[..length]).expect("Send indication"),
            turn::PeerData {
                peer,
                data: &payload,
            }
        );
        assert!(driver.deferred_relay.is_empty());
    }

    #[tokio::test]
    async fn the_driver_keeps_permissions_and_allocations_alive_and_deletes_on_shutdown() {
        let server = UdpSocket::bind("127.0.0.1:0".parse::<SocketAddr>().unwrap())
            .await
            .expect("relay binds");
        let server_address = server.local_addr().unwrap();
        let media = Arc::new(
            UdpSocket::bind("127.0.0.1:0".parse::<SocketAddr>().unwrap())
                .await
                .expect("media binds"),
        );
        let media_address = media.local_addr().unwrap();
        let credentials = Credentials::new("aaaa", "asd88fgpdd777uzjYhagZg").unwrap();
        let mut agent = Agent::new(super::super::Config::default(), true, credentials, 7);
        agent.handle(Input::LocalCandidate(super::super::Gathered::new(
            LocalBase(0),
            media_address,
            media_address,
            CandidateType::Host,
            ComponentId::RTP,
            None,
        )));
        agent.handle(Input::RemoteDescription {
            credentials: Credentials::new("bbbb", "zsd88fgpdd777uzjYhagZg").unwrap(),
            candidates: vec![
                Candidate::parse("1 1 UDP 2130706431 203.0.113.9 40000 typ host").unwrap(),
            ],
            lite: false,
        });
        let relay = turn::Relay::new(server_address, "1000", "relay-password").unwrap();
        let allocation = Allocation {
            base: LocalBase(0),
            server: server_address,
            relayed: "192.0.2.44:49152".parse().unwrap(),
            mapped: "198.51.100.20:50000".parse().unwrap(),
            lifetime: Duration::from_secs(600),
            auth: turn::Auth::challenged(&relay, "example.com", "nonce".to_owned(), None)
                .expect("legacy auth"),
        };
        let (_events, event_rx) = mpsc::channel(1);
        let (selection, _selected) = watch::channel(Selection::Checking);
        let routes = Arc::new(StdRwLock::new(Vec::new()));
        let stop = Arc::new(crate::session::Stop::default());
        let mut driver = Driver {
            agent,
            sockets: vec![media],
            deadlines: HashMap::new(),
            events: event_rx,
            destinations: Destinations {
                rtp: Arc::new(Mutex::new("203.0.113.9:40000".parse().unwrap())),
                rtcp: Arc::new(Mutex::new(None)),
            },
            selected: Arc::new(AtomicBool::new(false)),
            path: Arc::new(AtomicU8::new(IcePath::Checking.encoded())),
            selection,
            allocations: vec![allocation],
            allocation_refresh: Vec::new(),
            permission_refresh: None,
            turn_pending: Vec::new(),
            permissions: HashSet::new(),
            deferred_relay: Vec::new(),
            relay_routes: routes,
            stop,
            discards: Arc::new(DiscardMeters::default()),
        };

        driver.start_turn().await;
        let mut datagram = vec![0u8; 1500];
        let (length, _) =
            tokio::time::timeout(Duration::from_secs(1), server.recv_from(&mut datagram))
                .await
                .expect("permission send is bounded")
                .expect("permission arrives");
        assert_eq!(
            turn::operation_for_test(&datagram[..length]),
            Some("permission")
        );
        let permission_transaction =
            turn::transaction_for_test(&datagram[..length]).expect("transaction");
        driver
            .retransmit_turn(tokio::time::Instant::now() + TURN_RTO)
            .await;
        let (length, _) =
            tokio::time::timeout(Duration::from_secs(1), server.recv_from(&mut datagram))
                .await
                .expect("permission retransmit is bounded")
                .expect("permission retransmit arrives");
        assert_eq!(
            turn::transaction_for_test(&datagram[..length]),
            Some(permission_transaction),
            "a retransmission keeps the transaction id"
        );
        assert_eq!(driver.turn_pending[0].rto, Duration::from_secs(1));
        driver.turn_pending.clear();

        driver
            .service_turn(tokio::time::Instant::now() + Duration::from_secs(301))
            .await;
        let mut operations = Vec::new();
        for _ in 0..2 {
            let (length, _) =
                tokio::time::timeout(Duration::from_secs(1), server.recv_from(&mut datagram))
                    .await
                    .expect("refresh sends are bounded")
                    .expect("refresh arrives");
            operations.push(turn::operation_for_test(&datagram[..length]));
        }
        assert!(operations.contains(&Some("refresh")));
        assert!(operations.contains(&Some("permission")));

        driver.delete_allocations().await;
        let (length, _) =
            tokio::time::timeout(Duration::from_secs(1), server.recv_from(&mut datagram))
                .await
                .expect("delete send is bounded")
                .expect("delete arrives");
        assert_eq!(
            turn::operation_for_test(&datagram[..length]),
            Some("refresh")
        );
        assert_eq!(
            turn::lifetime_for_test(&datagram[..length]),
            Some(Duration::ZERO)
        );

        driver.turn_pending.clear();
        let now = tokio::time::Instant::now();
        for index in 0..=TURN_TRANSACTIONS {
            let octet = u8::try_from(index).expect("bound fits in one octet");
            let pending = TurnPending {
                base: LocalBase(0),
                transaction: [octet; 12],
                allocation: 0,
                operation: TurnOperation::Permission(IpAddr::V4(std::net::Ipv4Addr::new(
                    192, 0, 2, octet,
                ))),
                bytes: Vec::new(),
                retry_at: now,
                rto: TURN_RTO,
                retransmits: TURN_RETRANSMITS,
            };
            let _ = driver.track_turn(pending);
        }
        assert_eq!(driver.turn_pending.len(), TURN_TRANSACTIONS);
    }
}
