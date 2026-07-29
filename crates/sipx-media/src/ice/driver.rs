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
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::net::UdpSocket;
use tokio::sync::{Mutex, mpsc};

use sipx_sdp::ice::ComponentId;

use super::agent::{Agent, Input, Output, Timer};
use super::candidate::LocalBase;

/// How many events may queue for the driver before the media path stops offering them.
///
/// Small on purpose. Everything on this channel is either a datagram that has already been read
/// off the socket or a note that a media packet went out, and none of it is worth blocking a
/// receive loop or a send loop for: a driver that has fallen this far behind will not catch up by
/// being given more.
const EVENTS: usize = 64;

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
}

/// The handle the media path holds: where to send events, and whether it is worth sending them.
#[derive(Debug, Clone)]
pub(crate) struct Handle {
    events: mpsc::Sender<Event>,
    /// Whether a pair has been selected for component 1.
    ///
    /// Read by the send loop before it reports a packet, so that the fifty notes a second an
    /// ordinary call would produce are not even constructed until there is a selected pair for
    /// them to be about. §11's keepalive is only ever on a selected pair, so before there is one
    /// the agent would discard every one of them.
    selected: Arc<AtomicBool>,
}

impl Handle {
    /// Hand the driver a datagram. Non-blocking: a full queue drops it.
    ///
    /// Dropping is right and not merely convenient. A connectivity check is a retransmitted
    /// transaction (RFC 5389 §7.2.1) and the far end will send it again; blocking the receive loop
    /// on a slow driver would stall the *audio* to protect a check that is already redundant.
    pub(crate) fn datagram(&self, from: SocketAddr, on: LocalBase, bytes: Vec<u8>) {
        if self.events.try_send(Event::Datagram { from, on, bytes }).is_err() {
            tracing::debug!(%from, "dropping a connectivity check the ice driver could not take");
        }
    }

    /// Note that media went out, if there is a selected pair for it to have gone out on.
    pub(crate) fn data_sent(&self, component: ComponentId) {
        if !self.selected.load(Ordering::Relaxed) {
            return;
        }
        // A dropped note costs one keepalive that did not need to be sent; §11's indication is
        // unauthenticated and draws no response, so it is the cheapest thing here to lose.
        let _ = self.events.try_send(Event::DataSent { component });
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
    stop: Arc<crate::session::Stop>,
}

/// Start the driver for a stream, and hand the media path its end of it.
pub(crate) fn spawn(
    agent: Agent,
    pending: Vec<Output>,
    sockets: Vec<Arc<UdpSocket>>,
    destinations: Destinations,
    stop: Arc<crate::session::Stop>,
) -> Handle {
    let (events_tx, events_rx) = mpsc::channel(EVENTS);
    let selected = Arc::new(AtomicBool::new(false));
    let driver = Driver {
        agent,
        sockets,
        deadlines: HashMap::new(),
        events: events_rx,
        destinations,
        selected: Arc::clone(&selected),
        stop,
    };
    tokio::spawn(driver.run(pending));
    Handle {
        events: events_tx,
        selected,
    }
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

            let event = match next {
                Some((timer, at)) => tokio::select! {
                    () = self.stop.wait() => return,
                    event = self.events.recv() => event,
                    () = tokio::time::sleep_until(at) => {
                        // A one-shot that has fired is no longer armed. Removing it *before* the
                        // agent sees it is what makes the next one the agent's to ask for.
                        self.deadlines.remove(&timer);
                        let outputs = self.agent.handle(Input::TimerFired(timer));
                        self.apply(outputs).await;
                        continue;
                    }
                },
                None => tokio::select! {
                    () = self.stop.wait() => return,
                    event = self.events.recv() => event,
                },
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
            };
            self.apply(outputs).await;
        }
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
                        tracing::warn!(base = on.0, "no socket for the base the agent named");
                        continue;
                    };
                    if let Err(error) = socket.send_to(&bytes, to).await {
                        // One unreachable candidate is an ordinary thing to find — it is what
                        // checking is for — and the pair fails on its own timer rather than here.
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
                    remote,
                } => self.select(component, local, remote).await,
                Output::Failed { component } => {
                    // The call layer decides what a failed component means ([spec] §2). What the
                    // media path does is nothing: the stream keeps sending to the default
                    // destination, which is where it was already sending.
                    tracing::warn!(component = component.get(), "ice failed for a component");
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
    async fn select(&mut self, component: ComponentId, local: LocalBase, remote: SocketAddr) {
        if component == ComponentId::RTP {
            if local != LocalBase(0) {
                // RTP leaves the media socket, which is base 0 by construction. A selected pair
                // on any other base would mean audio and its checks on different sockets, and
                // the far end would see media from an address it never validated.
                tracing::warn!(base = local.0, "a selected rtp pair on a base that is not the media socket");
                return;
            }
            *self.destinations.rtp.lock().await = remote;
            self.selected.store(true, Ordering::Relaxed);
        } else {
            *self.destinations.rtcp.lock().await = Some(remote);
        }
        tracing::debug!(component = component.get(), %remote, "ice selected a pair");
    }
}
