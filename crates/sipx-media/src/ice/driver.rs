//! The driver: the socket and the clock the sans-IO agent does without ([spec] §2, §15).
//!
//! The division is the whole point of the shape. [`Agent`](super::Agent) owns the protocol —
//! which check goes out next, when it is retransmitted, which pair wins — and this task owns the
//! two things a state machine must not: a `UdpSocket` and a deadline. Every datagram it sends is
//! an [`Output::Send`] it was handed, and **every timer it arms is an [`Output::SetTimer`] it was
//! handed**. It never schedules anything of its own.
//!
//! That last rule is not tidiness. A driver with a timer of its own can keep an agent that has
//! stopped asking for ticks alive, which makes a dead pacing path look healthy from the outside —
//! the exact defect `M-21`'s review found in a *test* that fired Ta by hand, one layer up. The
//! deadline table here holds only what the agent put in it, a fired one-shot is removed before the
//! agent sees it, and nothing re-arms it but the agent's own next output.
//!
//! The other rule is subtractive: the driver feeds the agent only inputs [spec] §2 names, and only
//! when the thing they describe actually happened. Manufacturing an input — replaying a datagram,
//! synthesising a `DataSent` for media that did not go out — is how an outside caller reintroduces
//! the triggered-check storm `da9d49f` fixed on the inside.
//!
//! [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use tokio::net::UdpSocket;
use tokio::sync::{Mutex, mpsc, oneshot, watch};

use sipx_sdp::ice::{Candidate, CandidateType, ComponentId, Credentials};

use super::agent::{Agent, Input, Output, Timer};
use super::candidate::LocalBase;
use crate::counters::DiscardMeters;

/// How many events may queue for the driver before the media path stops offering them.
///
/// Small on purpose. Everything on this channel is either a datagram that has already been read
/// off the socket or a note that a media packet went out, and none of it is worth blocking a
/// receive loop or a send loop for: a driver that has fallen this far behind will not catch up by
/// being given more.
const EVENTS: usize = 64;

/// The candidate path an ICE-backed media session actually selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
/// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
#[derive(Debug, Clone)]
pub struct Local {
    /// `a=ice-ufrag` and `a=ice-pwd`, read back from the agent rather than from the caller's copy.
    pub credentials: Credentials,
    /// `a=candidate`, priced by the agent, in descending priority.
    pub candidates: Vec<Candidate>,
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
    pub(crate) fn datagram(&self, from: SocketAddr, on: LocalBase, bytes: Vec<u8>) -> bool {
        if self
            .events
            .try_send(Event::Datagram { from, on, bytes })
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
    stop: Arc<crate::session::Stop>,
    discards: Arc<DiscardMeters>,
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
    sockets: Vec<Arc<UdpSocket>>,
    destinations: Destinations,
    stop: Arc<crate::session::Stop>,
    discards: Arc<DiscardMeters>,
) -> Handle {
    let (handle, _task) = spawn_parts(agent, pending, sockets, destinations, stop, discards, None);
    handle
}

#[cfg(feature = "dtls")]
pub(crate) fn spawn_owned(
    agent: Agent,
    pending: Vec<Output>,
    sockets: Vec<Arc<UdpSocket>>,
    destinations: Destinations,
    stop: Arc<crate::session::Stop>,
    discards: Arc<DiscardMeters>,
    profile_tasks: Arc<crate::browser::ProfileTasks>,
) -> OwnedDriver {
    let (handle, task) = spawn_parts(
        agent,
        pending,
        sockets,
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
    sockets: Vec<Arc<UdpSocket>>,
    destinations: Destinations,
    stop: Arc<crate::session::Stop>,
    discards: Arc<DiscardMeters>,
    #[cfg_attr(not(feature = "dtls"), allow(unused_variables))] profile_tasks: Option<
        Arc<crate::browser::ProfileTasks>,
    >,
) -> (Handle, tokio::task::JoinHandle<()>) {
    let (events_tx, events_rx) = mpsc::channel(EVENTS);
    let selected = Arc::new(AtomicBool::new(false));
    let path = Arc::new(AtomicU8::new(IcePath::Checking.encoded()));
    let (selection, selected_pair) = watch::channel(Selection::Checking);
    let driver = Driver {
        agent,
        sockets,
        deadlines: HashMap::new(),
        events: events_rx,
        destinations,
        selected: Arc::clone(&selected),
        path: Arc::clone(&path),
        selection,
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
        discards,
    };
    (handle, task)
}

impl Driver {
    /// The loop. One `select!` over three things: the stop signal, an event from the media path,
    /// and the earliest deadline the agent has asked for.
    async fn run(mut self, pending: Vec<Output>) {
        self.apply(pending).await;

        loop {
            if self.stop.is_stopped() {
                return;
            }
            // Recomputed every pass, because the agent may have moved, cleared or added a
            // deadline while handling the last event. There is no timer here that outlives the
            // pass that armed it.
            let next = self
                .deadlines
                .iter()
                .min_by_key(|(_, at)| **at)
                .map(|(timer, at)| (*timer, *at));

            // Disabled outright when the agent has asked for nothing, so an agent that has gone
            // quiet is not woken by a deadline this loop invented. The instant in that case is
            // never waited on — the guard is what makes the arm inert.
            let deadline = next.map_or_else(tokio::time::Instant::now, |(_, at)| at);
            let event = tokio::select! {
                () = self.stop.wait() => return,
                event = self.events.recv() => event,
                () = tokio::time::sleep_until(deadline), if next.is_some() => {
                    if let Some((timer, _)) = next {
                        // A one-shot that has fired is no longer armed. Removing it *before* the
                        // agent sees it is what makes the next one the agent's to ask for.
                        self.deadlines.remove(&timer);
                        let outputs = self.agent.handle(Input::TimerFired(timer));
                        self.apply(outputs).await;
                    }
                    continue;
                }
            };

            let Some(event) = event else {
                // Every sender is gone, which means the session's loops have ended.
                return;
            };
            let outputs = match event {
                Event::Datagram { from, on, bytes } => {
                    self.agent.handle(Input::Datagram { from, on, bytes })
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
                Output::Send { on, to, bytes } => {
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
        tracing::debug!(component = component.get(), %remote, "ice selected a pair");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
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
}
