//! The ICE agent: a state machine over events (RFC 8445 §6–§8, §11; [spec] §2, §6–§10).
//!
//! Sans-IO, and that is a constraint from the working agreement rather than a preference. The
//! agent reads no clock, owns no socket and holds no `tokio` type. Time arrives as
//! [`Input::TimerFired`] and leaves as [`Output::SetTimer`]; datagrams arrive as bytes with a
//! source address and leave as bytes with a destination. Everything that makes ICE hard to get
//! right — pacing, retransmission, the order two agents converge in — is therefore reachable from
//! an ordinary unit test with no sleeping and no flakiness, which is the only way the seven rows
//! of §7.3.1.1's role-conflict table can each be asserted.
//!
//! What is *not* here, deliberately:
//!
//! - **Aggressive nomination.** RFC 8445 §4 deprecated it and §8.1.1 explains why it is no longer
//!   even useful — "in this specification, data can always be sent on any valid pair, without
//!   nomination". There is no option to enable it, because an option to enable it is an option to
//!   re-nominate mid-session, which is the behaviour `a=ice-options:ice2` exists to stop. The
//!   controlled side still tolerates a peer that nominates more than once, by selecting the
//!   highest-priority nominated pair: tolerating a legacy peer is not the same as being one.
//! - **The lite role.** [spec] §12, with the reason. Interoperating with a lite *peer* is in
//!   scope and is why [`Input::RemoteDescription`] carries `lite`.
//! - **Trickle ICE, TURN, and the socket.** The first two are out of scope for the spec; the
//!   third is the driver's, and the driver is a loop over [`Agent::handle`].
//!
//! [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md

use std::net::SocketAddr;
use std::time::Duration;

use sipx_sdp::ice::{Candidate, CandidateType, ComponentId, Credentials, Priority};

use super::candidate::{
    Foundations, Gathered, LocalBase, LocalCandidate, LocalId, PairFoundation, RemoteCandidate,
    RemoteId, assign_local_preferences,
};
use super::checklist::{
    CandidatePair, Checklist, ChecklistSet, ChecklistState, PairId, PairIds, PairState, Role,
    ValidPair, form_pairs, ordered_pair_priority,
};
use super::stun::{self, Class, Message, Peering, RoleAttribute, TransactionId};
use super::timing::Timers;

/// RFC 8445 §6.1.2.5's default limit on the size of the checklist set.
pub const DEFAULT_PAIR_LIMIT: usize = 100;

/// Only UDP: [spec] §3, and the transport is part of §5.1.1.3's foundation.
///
/// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
const TRANSPORT: sipx_sdp::ice::Transport = sipx_sdp::ice::Transport::Udp;

/// What the agent may be waiting for.
///
/// There is one retransmission timer per outstanding check rather than one for the agent, because
/// §14.3's RTO is per transaction: two checks sent one Ta apart have different retransmission
/// intervals, since the number of outstanding checks changed between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Timer {
    /// Ta: the pacing tick. One check leaves per tick, across the whole checklist set (§14.2).
    Ta,
    /// The retransmission timer for the check outstanding on this pair (RFC 5389 §7.2.1).
    Retransmit(PairId),
    /// Tn: how long the controlling agent keeps checking after the first valid pair appears
    /// before it nominates ([spec] §8).
    ///
    /// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
    Nomination,
    /// Tr: the keepalive interval on the selected pairs (§11).
    Keepalive,
}

/// Something that happened.
#[derive(Debug, Clone)]
pub enum Input {
    /// The far end's ICE parameters, from an offer or an answer.
    RemoteDescription {
        /// Its `a=ice-ufrag` and `a=ice-pwd` (RFC 8839 §5.4).
        credentials: Credentials,
        /// Its `a=candidate` lines. Ones naming a transport sipx does not check over are
        /// discarded here rather than failing the description.
        candidates: Vec<Candidate>,
        /// Whether it said `a=ice-lite` (RFC 8839 §5.3). A lite peer never sends a check, and
        /// §6.1.1 makes a full agent facing one controlling unconditionally.
        lite: bool,
    },
    /// A local candidate the driver gathered.
    LocalCandidate(Gathered),
    /// Gathering will produce nothing further.
    GatheringDone,
    /// A datagram [`crate::dtls::classify`] called STUN, and where it came from.
    Datagram {
        /// Its source address.
        from: SocketAddr,
        /// The socket it arrived on.
        on: LocalBase,
        /// The bytes.
        bytes: Vec<u8>,
    },
    /// Media went out on a selected pair; resets that pair's keepalive timer (§11).
    DataSent {
        /// Which component's selected pair carried it.
        component: ComponentId,
    },
    /// A timer fired.
    TimerFired(Timer),
}

/// Something the driver must do, in the order given.
///
/// A `Send` always precedes the `SetTimer` that will retransmit it — the same rule the transaction
/// machines follow, so a retransmission timer can never start before the thing it retransmits has
/// gone out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    /// Put these bytes on the wire. The driver owns the socket.
    Send {
        /// Which socket to send from.
        on: LocalBase,
        /// Where to send them.
        to: SocketAddr,
        /// The datagram.
        bytes: Vec<u8>,
    },
    /// Arrange for this timer to fire after this long.
    SetTimer {
        /// Which timer.
        timer: Timer,
        /// How long from now.
        after: Duration,
    },
    /// Cancel a timer that has not fired.
    ClearTimer(Timer),
    /// A component has a selected pair: media goes here now, in both directions.
    Selected {
        /// Which component.
        component: ComponentId,
        /// The socket to send it from.
        local: LocalBase,
        /// The address to send it to.
        remote: SocketAddr,
    },
    /// ICE failed for a component. The call layer decides what that means.
    Failed {
        /// Which component.
        component: ComponentId,
    },
}

/// Everything about the agent a deployment may change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// The timers of §14 and [spec] §9.
    ///
    /// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
    pub timers: Timers,
    /// §6.1.2.5's limit on the size of the checklist set. "The default limit … is 100, but the
    /// value MUST be configurable."
    pub pair_limit: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            timers: Timers::default(),
            pair_limit: DEFAULT_PAIR_LIMIT,
        }
    }
}

/// One outstanding connectivity check (RFC 5389 §7.2.1, RFC 8445 §7.2).
#[derive(Debug, Clone)]
struct Transaction {
    id: TransactionId,
    pair: PairId,
    on: LocalBase,
    /// The local address the request went out from — half of §7.2.5.2.1's symmetry test.
    from: SocketAddr,
    /// The address it was sent to — the other half.
    to: SocketAddr,
    /// The exact bytes, kept so a retransmission is a retransmission and not a second message
    /// with the same transaction ID.
    bytes: Vec<u8>,
    /// The `PRIORITY` this check claimed, which §7.2.5.3.1 makes the priority of any
    /// peer-reflexive candidate the response teaches us.
    priority: Priority,
    /// Which role attribute went out, which is what §7.2.5.1 reads to decide which way to switch.
    role: RoleAttribute,
    /// Whether this check carried `USE-CANDIDATE`.
    nominating: bool,
    attempt: u32,
    rto: Duration,
    initial_rto: Duration,
    final_wait: bool,
    /// §7.3.1.4's cancellation: no more retransmissions and no failure on silence, but the
    /// response is still accepted if it arrives.
    cancelled: bool,
}

/// What §7.3.1.1 makes of an inbound check's role attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Conflict {
    /// No conflict: the peer named the other role, or named none at all.
    None,
    /// Our role changed. §7.3.1.1's remaining processing still runs.
    Switched,
    /// Answer 487 Role Conflict and keep our role. Nothing else in §7.3.1 runs.
    Reject,
}

/// How far [spec] §8's stopping criterion has got.
///
/// `Tn` is armed by the first valid pair and not by the first check, because the criterion is
/// "how long the controlling agent keeps checking **after the first valid pair**": arming it
/// earlier nominates before there is anything to nominate.
///
/// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stopping {
    /// No valid pair yet, so nothing is being waited for.
    Idle,
    /// A valid pair appeared and `Tn` is running.
    Armed,
    /// `Tn` fired: nominate the best valid pair for each component now.
    Elapsed,
}

/// A component that has been concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Selection {
    component: ComponentId,
    local: LocalBase,
    remote: SocketAddr,
    priority: u64,
}

/// The sans-IO ICE agent.
///
/// One agent per data stream, which is one checklist. The checklist *set* it holds is still a set
/// — see [`super::checklist`] — because §6.1.2.6's unfreezing rule is stated over the set.
#[derive(Debug)]
pub struct Agent {
    config: Config,
    role: Role,
    tiebreaker: u64,
    offerer: bool,
    credentials: Credentials,
    peering: Option<Peering>,
    local: Vec<LocalCandidate>,
    remote: Vec<RemoteCandidate>,
    foundations: Foundations,
    ids: PairIds,
    set: ChecklistSet,
    transactions: Vec<Transaction>,
    gathering_done: bool,
    started: bool,
    /// Pairs the controlling agent has enqueued a nominating check for (§8.1.1). A component
    /// appears here once and once only: "the agent MUST NOT nominate another pair for [the] same
    /// component … within the ICE session".
    nominating: Vec<(ComponentId, PairId)>,
    /// Pairs whose triggered check was caused by a `USE-CANDIDATE` we accepted while controlled
    /// (§7.3.1.5), and whose success therefore nominates.
    nominate_on_success: Vec<PairId>,
    /// Where [spec] §8's `Tn` has got to.
    ///
    /// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
    stopping: Stopping,
    selected: Vec<Selection>,
    failed: Vec<ComponentId>,
}

impl Agent {
    /// A new agent.
    ///
    /// `offerer` is whether sipx sent the initial offer, which is §6.1.1's role determination for
    /// two full agents; `tiebreaker` is §7.1.3's 64-bit value, chosen at random per ICE session by
    /// the caller — a machine that reads no clock does not reach for an RNG behind the caller's
    /// back either, and a test that wants the `T = V` row of §7.3.1.1's table needs to choose it.
    #[must_use]
    pub fn new(config: Config, offerer: bool, credentials: Credentials, tiebreaker: u64) -> Self {
        Self {
            config,
            role: Role::determine(offerer, false),
            tiebreaker,
            offerer,
            credentials,
            peering: None,
            local: Vec::new(),
            remote: Vec::new(),
            foundations: Foundations::default(),
            ids: PairIds::default(),
            set: ChecklistSet::new(),
            transactions: Vec::new(),
            gathering_done: false,
            started: false,
            nominating: Vec::new(),
            nominate_on_success: Vec::new(),
            stopping: Stopping::Idle,
            selected: Vec::new(),
            failed: Vec::new(),
        }
    }

    /// Our role (§6.1.1), which §7.3.1.1 and §7.2.5.1 may change.
    #[must_use]
    pub const fn role(&self) -> Role {
        self.role
    }

    /// Our tiebreaker (§7.1.3). It changes when a 487 does (§7.2.5.1).
    #[must_use]
    pub const fn tiebreaker(&self) -> u64 {
        self.tiebreaker
    }

    /// The checklist set.
    #[must_use]
    pub const fn checklists(&self) -> &ChecklistSet {
        &self.set
    }

    /// The local candidates, including any peer-reflexive one §7.2.5.3.1 learned.
    #[must_use]
    pub fn local_candidates(&self) -> &[LocalCandidate] {
        &self.local
    }

    /// The remote candidates, including any peer-reflexive one §7.3.1.3 learned.
    #[must_use]
    pub fn remote_candidates(&self) -> &[RemoteCandidate] {
        &self.remote
    }

    /// The selected pair for a component, once there is one (§8.1.1).
    #[must_use]
    pub fn selected(&self, component: ComponentId) -> Option<(LocalBase, SocketAddr)> {
        self.selected
            .iter()
            .find(|selection| selection.component == component)
            .map(|selection| (selection.local, selection.remote))
    }

    /// Feed the agent an event and get back what the driver must do.
    pub fn handle(&mut self, input: Input) -> Vec<Output> {
        let mut out = Vec::new();
        match input {
            Input::RemoteDescription {
                credentials,
                candidates,
                lite,
            } => self.remote_description(credentials, &candidates, lite, &mut out),
            Input::LocalCandidate(gathered) => self.local_candidate(gathered),
            Input::GatheringDone => {
                self.gathering_done = true;
                self.start(&mut out);
            }
            Input::Datagram { from, on, bytes } => self.datagram(from, on, &bytes, &mut out),
            Input::DataSent { component } => {
                if self.selected(component).is_some() {
                    out.push(Output::SetTimer {
                        timer: Timer::Keepalive,
                        after: self.config.timers.tr,
                    });
                }
            }
            Input::TimerFired(timer) => self.timer(timer, &mut out),
        }
        out
    }

    // ---------------------------------------------------------------- gathering and description

    fn remote_description(
        &mut self,
        credentials: Credentials,
        candidates: &[Candidate],
        lite: bool,
        out: &mut Vec<Output>,
    ) {
        // §6.1.1, and the peer's `a=ice-lite` is the whole reason this is not decided in the
        // constructor: a full agent facing a lite one controls whoever offered.
        self.role = Role::determine(self.offerer, lite);
        self.peering = Some(Peering::new(self.credentials.clone(), credentials));
        self.remote = candidates
            .iter()
            .filter_map(RemoteCandidate::signalled)
            .collect();
        self.start(out);
    }

    fn local_candidate(&mut self, gathered: Gathered) {
        let foundation = self.foundations.assign(&gathered, TRANSPORT);
        self.local.push(LocalCandidate {
            gathered,
            foundation,
            local_preference: 0,
            priority: Priority::MIN,
        });
        // §5.1.2.1's local preference is a property of the set, so every candidate is repriced
        // whenever the set grows.
        assign_local_preferences(&mut self.local);
    }

    /// §6.1.2: form the checklists once both halves of the exchange are in.
    fn start(&mut self, out: &mut Vec<Output>) {
        if self.started || !self.gathering_done || self.peering.is_none() {
            return;
        }
        if self.local.is_empty() || self.remote.is_empty() {
            return;
        }
        self.started = true;
        let pairs = form_pairs(&mut self.ids, self.role, &self.local, &self.remote);
        self.set = ChecklistSet::new();
        self.set.push(Checklist::new(pairs));
        self.set.limit(self.config.pair_limit);
        self.set.compute_initial_states();
        out.push(Output::SetTimer {
            timer: Timer::Ta,
            after: self.config.timers.pacing(),
        });
    }

    // ------------------------------------------------------------------------------- the timers

    fn timer(&mut self, timer: Timer, out: &mut Vec<Output>) {
        match timer {
            Timer::Ta => self.pace(out),
            Timer::Retransmit(pair) => self.retransmit(pair, out),
            Timer::Nomination => {
                self.stopping = Stopping::Elapsed;
                self.nominate();
            }
            Timer::Keepalive => self.keepalive(out),
        }
    }

    /// §6.1.4.2: one check per Ta tick, taken from the next checklist in the Running state.
    fn pace(&mut self, out: &mut Vec<Output>) {
        let count = self.set.checklists().len();
        for _ in 0..count {
            let Some(index) = self.set.next_running() else {
                break;
            };
            // Step 1: the triggered-check queue first, whatever its pairs' priorities are. This
            // is what makes ICE converge in the time it takes a peer's check to arrive.
            let triggered = self
                .set
                .checklists_mut()
                .get_mut(index)
                .and_then(Checklist::take_triggered);
            if let Some(id) = triggered {
                self.send_check(id, out);
                break;
            }
            // Step 2: nothing Waiting here, so thaw what the set allows.
            self.set.unfreeze_idle(index);
            // Step 3: the highest-priority Waiting pair, ties broken by the lowest component.
            let waiting = self.set.checklists().get(index).and_then(|list| {
                list.pairs()
                    .iter()
                    .filter(|pair| pair.state == PairState::Waiting)
                    .max_by_key(|pair| (pair.priority, std::cmp::Reverse(pair.component)))
                    .map(|pair| pair.id)
            });
            if let Some(id) = waiting {
                self.send_check(id, out);
                break;
            }
            // Step 4: nothing to do for this checklist; try the next one without waiting for Ta.
        }
        if self.running() {
            out.push(Output::SetTimer {
                timer: Timer::Ta,
                after: self.config.timers.pacing(),
            });
        }
    }

    fn running(&self) -> bool {
        self.set
            .checklists()
            .iter()
            .any(|list| list.state() == ChecklistState::Running)
    }

    /// RFC 5389 §7.2.1: Rc transmissions, doubling the interval each time, then a final wait of
    /// Rm times the RTO before the transaction has timed out.
    fn retransmit(&mut self, pair: PairId, out: &mut Vec<Output>) {
        let Some(position) = self
            .transactions
            .iter()
            .position(|transaction| transaction.pair == pair)
        else {
            return;
        };
        let Some(transaction) = self.transactions.get_mut(position) else {
            return;
        };
        if transaction.cancelled {
            // §7.3.1.4: a cancelled transaction is not retransmitted and its silence is not a
            // failure. The pair already has a triggered check of its own.
            self.transactions.remove(position);
            return;
        }
        if transaction.attempt < self.config.timers.rc {
            transaction.attempt = transaction.attempt.saturating_add(1);
            transaction.rto = self.config.timers.double(transaction.rto);
            let (on, to, bytes, after) = (
                transaction.on,
                transaction.to,
                transaction.bytes.clone(),
                transaction.rto,
            );
            out.push(Output::Send { on, to, bytes });
            out.push(Output::SetTimer {
                timer: Timer::Retransmit(pair),
                after,
            });
            return;
        }
        if !transaction.final_wait {
            transaction.final_wait = true;
            let after = self.config.timers.final_wait(transaction.initial_rto);
            out.push(Output::SetTimer {
                timer: Timer::Retransmit(pair),
                after,
            });
            return;
        }
        // §7.2.5.2.3: the transaction timed out, so the pair failed.
        let nominating = transaction.nominating;
        self.transactions.remove(position);
        self.fail_pair(pair, nominating, out);
    }

    /// §11: a Binding Indication on each selected pair, holding the NAT binding open.
    fn keepalive(&mut self, out: &mut Vec<Output>) {
        for selection in &self.selected {
            if let Ok(bytes) = stun::keepalive(stun::new_transaction_id()) {
                out.push(Output::Send {
                    on: selection.local,
                    to: selection.remote,
                    bytes,
                });
            }
        }
        if !self.selected.is_empty() {
            out.push(Output::SetTimer {
                timer: Timer::Keepalive,
                after: self.config.timers.tr,
            });
        }
    }

    // -------------------------------------------------------------------------- sending a check

    fn send_check(&mut self, id: PairId, out: &mut Vec<Output>) {
        let Some(peering) = self.peering.clone() else {
            return;
        };
        let Some(pair) = self.set.pair(id) else {
            return;
        };
        let (local_id, remote_id, component) = (pair.local, pair.remote, pair.component);
        let (Some(local), Some(remote)) = (self.local.get(local_id.0), self.remote.get(remote_id.0))
        else {
            return;
        };
        let (on, from, to) = (
            local.gathered.base,
            local.gathered.base_address,
            remote.address,
        );
        // §7.1.1: the peer-reflexive preference, not the candidate's own.
        let check_priority = local.check_priority();
        let nominating = self.role.is_controlling()
            && self
                .nominating
                .iter()
                .any(|(nominated, pair)| *nominated == component && *pair == id);
        let role = match self.role {
            Role::Controlling => RoleAttribute::Controlling {
                tiebreaker: self.tiebreaker,
                nominate: nominating,
            },
            Role::Controlled => RoleAttribute::Controlled {
                tiebreaker: self.tiebreaker,
            },
        };
        let transaction_id = stun::new_transaction_id();
        let Ok(bytes) = stun::connectivity_check(transaction_id, &peering, check_priority, role)
        else {
            return;
        };

        if let Some(pair) = self.set.pair_mut(id) {
            pair.state = PairState::InProgress;
        }
        // §14.3, and the reason it is here and not in the constructor: the RTO counts the checks
        // outstanding *now*, including this one.
        let rto = self
            .config
            .timers
            .rto(self.set.total_pairs(), self.set.outstanding());

        out.push(Output::Send {
            on,
            to,
            bytes: bytes.clone(),
        });
        out.push(Output::SetTimer {
            timer: Timer::Retransmit(id),
            after: rto,
        });
        self.transactions
            .retain(|transaction| transaction.pair != id);
        self.transactions.push(Transaction {
            id: transaction_id,
            pair: id,
            on,
            from,
            to,
            bytes,
            priority: check_priority,
            role,
            nominating,
            attempt: 1,
            rto,
            initial_rto: rto,
            final_wait: false,
            cancelled: false,
        });
    }

    // ----------------------------------------------------------------------- inbound  datagrams

    fn datagram(&mut self, from: SocketAddr, on: LocalBase, bytes: &[u8], out: &mut Vec<Output>) {
        let Ok(message) = Message::decode(bytes) else {
            // [spec] §11.3: a malformed datagram is a dropped datagram, never a state change.
            return;
        };
        match message.class() {
            Class::Request => self.inbound_check(from, on, &message, out),
            Class::Success | Class::Error => self.inbound_response(from, on, &message, out),
            // §11's keepalive draws no response and means nothing to the state machine.
            Class::Indication => {}
        }
    }

    /// §7.3: sipx is a STUN server on the media port as well as a client.
    fn inbound_check(
        &mut self,
        from: SocketAddr,
        on: LocalBase,
        message: &Message,
        out: &mut Vec<Output>,
    ) {
        let Some(peering) = self.peering.clone() else {
            return;
        };
        // [spec] §11.2 and §11.3: the credential is checked before anything moves. An
        // unauthenticated check is dropped rather than answered — answering one tells an off-path
        // attacker which ufrag is live, and RFC 5389 §10.1.2's 401 is of no use to a peer that
        // never had our password.
        if message.username() != Some(peering.inbound_username().as_str())
            || !message.verify_integrity(peering.inbound_key())
        {
            return;
        }

        match self.resolve_conflict(message.role()) {
            Conflict::Reject => {
                if let Ok(bytes) = stun::role_conflict(message.transaction(), &peering) {
                    out.push(Output::Send { on, to: from, bytes });
                }
                return;
            }
            Conflict::Switched | Conflict::None => {}
        }

        // §7.3.1: the rest runs whether or not the role changed, so long as a success response is
        // generated — which it is, from here on.
        if let Ok(bytes) = stun::check_success(message.transaction(), &peering, from) {
            out.push(Output::Send { on, to: from, bytes });
        }
        if !self.started {
            // No checklist yet, so there is nothing to trigger. The response above still went, as
            // §7.3 requires of an agent that has published a candidate on this base.
            return;
        }

        let Some(local_id) = self.base_candidate(on) else {
            return;
        };
        let Some(component) = self
            .local
            .get(local_id.0)
            .map(|candidate| candidate.gathered.component)
        else {
            return;
        };
        let remote_id = self.learn_remote(from, component, message.priority());
        self.triggered_check(local_id, remote_id, component, message.use_candidate(), out);
    }

    /// §7.3.1.3: a check from an address no remote candidate names is a peer-reflexive remote
    /// candidate.
    fn learn_remote(
        &mut self,
        from: SocketAddr,
        component: ComponentId,
        claimed: Option<Priority>,
    ) -> RemoteId {
        if let Some(index) = self
            .remote
            .iter()
            .position(|candidate| candidate.address == from)
        {
            return RemoteId(index);
        }
        self.remote.push(RemoteCandidate {
            address: from,
            kind: CandidateType::PeerReflexive,
            component,
            // "an arbitrary value, different from the foundations of all other remote candidates"
            foundation: self.foundations.learn_remote(),
            // "the priority is the value of the PRIORITY attribute in the Binding request" — and
            // a check that carries none is violating §7.1.1, so it gets the floor rather than a
            // priority it did not claim.
            priority: claimed.unwrap_or(Priority::MIN),
        });
        RemoteId(self.remote.len().saturating_sub(1))
    }

    /// §7.3.1.4, and §7.3.1.5's nomination when the check that arrived carried `USE-CANDIDATE`.
    fn triggered_check(
        &mut self,
        local: LocalId,
        remote: RemoteId,
        component: ComponentId,
        use_candidate: bool,
        out: &mut Vec<Output>,
    ) {
        let existing = self
            .set
            .checklists()
            .iter()
            .find_map(|list| list.find(local, remote));
        let id = if let Some(id) = existing {
            id
        } else {
            // §7.3.1.4: "the pair is inserted into the checklist based on its priority. Its state
            // is set to Waiting. The pair is enqueued into the triggered-check queue."
            let Some(pair) = self.build_pair(local, remote, component) else {
                return;
            };
            let id = pair.id;
            if let Some(list) = self.set.checklists_mut().first_mut() {
                list.insert(pair);
                list.trigger(id);
            }
            if let Some(pair) = self.set.pair_mut(id) {
                pair.state = PairState::Waiting;
            }
            id
        };
        let before = self.set.pair(id).map(|pair| pair.state);
        match before {
            Some(PairState::Succeeded) => {
                // "If the state of that pair is Succeeded, nothing further is done."
            }
            Some(state) => {
                if state == PairState::InProgress {
                    // "the agent cancels the In-Progress transaction" — no more retransmissions
                    // and no failure on silence, but the response is still accepted.
                    if let Some(transaction) = self
                        .transactions
                        .iter_mut()
                        .find(|transaction| transaction.pair == id)
                    {
                        transaction.cancelled = true;
                    }
                }
                if let Some(pair) = self.set.pair_mut(id) {
                    pair.state = PairState::Waiting;
                }
                if let Some(index) = self.set.checklist_of(id)
                    && let Some(list) = self.set.checklists_mut().get_mut(index)
                {
                    list.trigger(id);
                }
            }
            None => return,
        }

        if use_candidate && !self.role.is_controlling() {
            // §7.3.1.5. A Succeeded pair is nominated now; anything else is nominated when the
            // triggered check this just enqueued succeeds.
            if before == Some(PairState::Succeeded) {
                self.mark_nominated(id, out);
            } else if !self.nominate_on_success.contains(&id) {
                self.nominate_on_success.push(id);
            }
        }
    }

    fn build_pair(
        &mut self,
        local: LocalId,
        remote: RemoteId,
        component: ComponentId,
    ) -> Option<CandidatePair> {
        let local_candidate = self.local.get(local.0)?;
        let remote_candidate = self.remote.get(remote.0)?;
        Some(CandidatePair {
            id: self.ids.allocate(),
            local,
            remote,
            component,
            foundation: PairFoundation {
                local: local_candidate.foundation,
                remote: remote_candidate.foundation.clone(),
            },
            priority: ordered_pair_priority(
                self.role,
                local_candidate.priority,
                remote_candidate.priority,
            ),
            state: PairState::Frozen,
            nominated: false,
        })
    }

    fn base_candidate(&self, on: LocalBase) -> Option<LocalId> {
        self.local
            .iter()
            .position(|candidate| {
                candidate.gathered.base == on && candidate.gathered.kind == CandidateType::Host
            })
            .map(LocalId)
    }

    // ------------------------------------------------------------------------------- a response

    fn inbound_response(
        &mut self,
        from: SocketAddr,
        on: LocalBase,
        message: &Message,
        out: &mut Vec<Output>,
    ) {
        let Some(peering) = self.peering.clone() else {
            return;
        };
        let Some(position) = self
            .transactions
            .iter()
            .position(|transaction| transaction.id == message.transaction())
        else {
            return;
        };
        let Some(transaction) = self.transactions.get(position).cloned() else {
            return;
        };
        // [spec] §11.3: an unauthenticated message moves nothing, including into Failed — or an
        // off-path attacker could fail every pair by answering the checks it can see.
        if !message.verify_integrity(peering.outbound_key()) {
            return;
        }
        // §7.2.5.2.1's symmetry test, before anything else is read: a response whose source is
        // not where the request went cannot be a response to it.
        if from != transaction.to || on != transaction.on {
            self.transactions.remove(position);
            out.push(Output::ClearTimer(Timer::Retransmit(transaction.pair)));
            self.fail_pair(transaction.pair, transaction.nominating, out);
            return;
        }

        self.transactions.remove(position);
        out.push(Output::ClearTimer(Timer::Retransmit(transaction.pair)));

        if message.class() == Class::Error {
            if message.error_code() == Some(stun::ROLE_CONFLICT) {
                self.role_conflict_response(&transaction);
            } else {
                // §7.2.5.2.4: an unrecoverable error response fails the pair.
                self.fail_pair(transaction.pair, transaction.nominating, out);
            }
            return;
        }

        self.success(&transaction, message, out);
    }

    /// §7.2.5.3: a check succeeded.
    fn success(&mut self, transaction: &Transaction, message: &Message, out: &mut Vec<Output>) {
        let Some(pair) = self.set.pair(transaction.pair).cloned() else {
            return;
        };
        // §7.2.5.3.1: the mapped address decides whether we just learned a candidate. A response
        // without one cannot have; treat it as the un-NATed case rather than as a failure, since
        // the pair demonstrably works either way.
        let mapped = message.mapped_address().unwrap_or(transaction.from);
        let local = self.learn_local(mapped, &pair, transaction.priority);

        // §7.2.5.3.2: the valid pair is built from the mapped address and the address the request
        // was sent to, which is very often not a pair in any checklist.
        let priority = self
            .local
            .get(local.0)
            .zip(self.remote.get(pair.remote.0))
            .map_or(pair.priority, |(local, remote)| {
                ordered_pair_priority(self.role, local.priority, remote.priority)
            });
        let first_valid = if let Some(index) = self.set.checklist_of(transaction.pair) {
            self.set
                .checklists_mut()
                .get_mut(index)
                .is_some_and(|list| {
                    list.add_valid(ValidPair {
                        component: pair.component,
                        local,
                        remote: transaction.to,
                        priority,
                        nominated: false,
                        generated_by: pair.id,
                    })
                })
        } else {
            false
        };

        // §7.2.5.3.3.
        if let Some(pair) = self.set.pair_mut(transaction.pair) {
            pair.state = PairState::Succeeded;
        }
        self.set.unfreeze_foundation(&pair.foundation);

        // §7.2.5.3.4, both directions: the check we nominated with, and the check a controlled
        // agent sent because the peer nominated.
        if transaction.nominating || self.nominate_on_success.contains(&transaction.pair) {
            self.nominate_on_success
                .retain(|id| *id != transaction.pair);
            self.mark_nominated(transaction.pair, out);
        }

        if first_valid && self.stopping == Stopping::Idle {
            // [spec] §8: Tn counts from the first valid pair, not from the first check.
            self.stopping = Stopping::Armed;
            out.push(Output::SetTimer {
                timer: Timer::Nomination,
                after: self.config.timers.tn,
            });
        }

        self.update_checklists(out);
        self.nominate();
    }

    /// §7.2.5.3.1: a mapped address that is not a local candidate is a peer-reflexive one.
    fn learn_local(&mut self, mapped: SocketAddr, pair: &CandidatePair, claimed: Priority) -> LocalId {
        if let Some(index) = self
            .local
            .iter()
            .position(|candidate| candidate.gathered.address == mapped)
        {
            return LocalId(index);
        }
        let Some(base) = self.local.get(pair.local.0).copied() else {
            return pair.local;
        };
        let gathered = Gathered {
            base: base.gathered.base,
            base_address: base.gathered.base_address,
            address: mapped,
            kind: CandidateType::PeerReflexive,
            component: pair.component,
            server: None,
        };
        let foundation = self.foundations.assign(&gathered, TRANSPORT);
        self.local.push(LocalCandidate {
            gathered,
            foundation,
            local_preference: base.local_preference,
            // "The priority is the value of the PRIORITY attribute in the Binding request" — the
            // one this agent sent, which §7.1.1 already computed with the peer-reflexive
            // preference. That is what makes both ends price this candidate the same.
            priority: claimed,
        });
        LocalId(self.local.len().saturating_sub(1))
    }

    fn fail_pair(&mut self, id: PairId, nominating: bool, out: &mut Vec<Output>) {
        if let Some(pair) = self.set.pair_mut(id) {
            pair.state = PairState::Failed;
        }
        if nominating {
            // §7.2.5.3.4: a nominated check that fails takes its valid pair and its checklist
            // with it. There is no second nomination to fall back on.
            if let Some(index) = self.set.checklist_of(id)
                && let Some(list) = self.set.checklists_mut().get_mut(index)
            {
                list.remove_valid(id);
                list.set_state(ChecklistState::Failed);
            }
        }
        self.update_checklists(out);
    }

    // ---------------------------------------------------------------------------- role conflict

    /// §7.3.1.1's table, applied to the role attribute on an inbound check.
    fn resolve_conflict(&mut self, attribute: Option<RoleAttribute>) -> Conflict {
        let Some(attribute) = attribute else {
            // The last row: the peer is not doing role signalling, so there is no conflict.
            return Conflict::None;
        };
        let _ = attribute;
        Conflict::None
    }

    /// §7.2.5.1: our own check drew a 487.
    fn role_conflict_response(&mut self, transaction: &Transaction) {
        let _ = transaction;
    }

    fn switch_role(&mut self) {
        self.role = self.role.opposite();
        // §7.3.1.1's NOTE: "A change in roles will require an agent to recompute pair priorities
        // (Section 6.1.2.3), since those priorities are a function of role."
        self.set
            .recompute_priorities(self.role, &self.local, &self.remote);
    }

    // ------------------------------------------------------------------------------- concluding

    /// §8.1.1's nomination, under [spec] §8's stopping criterion.
    ///
    /// Regular nomination and nothing else: the controlling agent picks a valid pair and repeats
    /// the check that produced it with `USE-CANDIDATE`, by enqueueing that pair on the
    /// triggered-check queue. Once a component is nominated it is never nominated again.
    ///
    /// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
    fn nominate(&mut self) {
        if !self.role.is_controlling() {
            return;
        }
        let mut queue: Vec<(usize, ComponentId, PairId)> = Vec::new();
        for (index, list) in self.set.checklists().iter().enumerate() {
            if list.state() != ChecklistState::Running {
                continue;
            }
            let components = list.components();
            if components.is_empty() {
                continue;
            }
            // "every component has at least one valid pair"
            let best: Vec<(ComponentId, &ValidPair)> = components
                .iter()
                .filter_map(|component| {
                    list.valid()
                        .iter()
                        .filter(|valid| valid.component == *component)
                        .max_by_key(|valid| valid.priority)
                        .map(|valid| (*component, valid))
                })
                .collect();
            if best.len() != components.len() {
                continue;
            }
            // "…and either every pair of higher priority than the best valid pair has reached
            // Failed, or Tn has elapsed since the first valid pair appeared."
            let settled = best.iter().all(|(component, valid)| {
                list.pairs()
                    .iter()
                    .filter(|pair| pair.component == *component && pair.priority > valid.priority)
                    .all(|pair| pair.state == PairState::Failed)
            });
            if !settled && self.stopping != Stopping::Elapsed {
                continue;
            }
            for (component, valid) in best {
                if self
                    .nominating
                    .iter()
                    .any(|(nominated, _)| *nominated == component)
                {
                    continue;
                }
                queue.push((index, component, valid.generated_by));
            }
        }
        for (index, component, pair) in queue {
            self.nominating.push((component, pair));
            if let Some(list) = self.set.checklists_mut().get_mut(index) {
                list.trigger(pair);
            }
        }
    }

    /// §7.2.5.3.4 and §7.3.1.5: this pair's valid pair is nominated, which concludes its
    /// component.
    fn mark_nominated(&mut self, id: PairId, out: &mut Vec<Output>) {
        let Some(index) = self.set.checklist_of(id) else {
            return;
        };
        if let Some(pair) = self.set.pair_mut(id) {
            pair.nominated = true;
        }
        if let Some(list) = self.set.checklists_mut().get_mut(index) {
            list.nominate_valid(id);
        }
        self.conclude(index, out);
    }

    /// §8.1.2: a nominated pair concludes its component, and a nominated pair for every component
    /// completes the checklist.
    fn conclude(&mut self, index: usize, out: &mut Vec<Output>) {
        let Some(list) = self.set.checklists().get(index) else {
            return;
        };
        let components = list.components();
        let mut chosen: Vec<(ComponentId, PairId, LocalId, SocketAddr, u64)> = Vec::new();
        for component in &components {
            // §8.1.1's tolerance for a peer that nominates more than once: "the agents MUST
            // produce the selected pairs and use the pairs with the highest priority". sipx never
            // nominates twice itself; this is what stops a peer that does from being obeyed.
            if let Some(valid) = list
                .valid()
                .iter()
                .filter(|valid| valid.component == *component && valid.nominated)
                .max_by_key(|valid| valid.priority)
            {
                chosen.push((
                    *component,
                    valid.generated_by,
                    valid.local,
                    valid.remote,
                    valid.priority,
                ));
            }
        }

        for (component, pair, local, remote, priority) in &chosen {
            let Some(base) = self
                .local
                .get(local.0)
                .map(|candidate| candidate.gathered.base)
            else {
                continue;
            };
            let selection = Selection {
                component: *component,
                local: base,
                remote: *remote,
                priority: *priority,
            };
            let existing = self
                .selected
                .iter_mut()
                .find(|known| known.component == *component);
            match existing {
                Some(known) if *known == selection => continue,
                Some(known) if known.priority >= *priority => continue,
                Some(known) => *known = selection,
                None => self.selected.push(selection),
            }
            out.push(Output::Selected {
                component: *component,
                local: base,
                remote: *remote,
            });
            if let Some(list) = self.set.checklists_mut().get_mut(index) {
                list.keep_only_nominated(*component, *pair);
            }
        }

        if chosen.len() == components.len() && !components.is_empty() {
            if let Some(list) = self.set.checklists_mut().get_mut(index) {
                list.set_state(ChecklistState::Completed);
            }
            self.transactions.clear();
            if !self.running() {
                out.push(Output::ClearTimer(Timer::Ta));
                out.push(Output::SetTimer {
                    timer: Timer::Keepalive,
                    after: self.config.timers.tr,
                });
            }
        }
    }

    /// §7.2.5.4: whether a checklist has finished, one way or the other.
    fn update_checklists(&mut self, out: &mut Vec<Output>) {
        let mut failed: Vec<ComponentId> = Vec::new();
        for list in self.set.checklists_mut() {
            if list.state() != ChecklistState::Running {
                continue;
            }
            let components = list.components();
            if components.is_empty() {
                continue;
            }
            let settled = list.pairs().iter().all(|pair| pair.state.is_final());
            let covered = components.iter().all(|component| {
                list.valid()
                    .iter()
                    .any(|valid| valid.component == *component)
            });
            if settled && !covered {
                list.set_state(ChecklistState::Failed);
                for component in components {
                    if !list
                        .valid()
                        .iter()
                        .any(|valid| valid.component == component)
                    {
                        failed.push(component);
                    }
                }
            }
        }
        for component in failed {
            if self.failed.contains(&component) {
                continue;
            }
            self.failed.push(component);
            out.push(Output::Failed { component });
        }
    }
}

/// A new tiebreaker after a 487 (§7.2.5.1: "the agent MUST change the tiebreaker value").
///
/// Random, and not a bump of the old value, and that is the whole difficulty of the symmetric
/// case. Two agents that both start controlling with the *same* tiebreaker each 487 the other and
/// each switch to controlled — and if both derive their new value the same way from the same old
/// value, they land on the same new value, compare equal again on the next check, both switch back
/// to controlling under §7.3.1.1's `T ≥ V` row, and ping-pong roles until the checklist fails.
/// Only an independent draw at each end breaks that symmetry, which is why §7.1.3 makes the
/// tiebreaker random in the first place.
///
/// The redraw on collision is not superstition about the RNG: §7.2.5.1 says the value MUST
/// *change*, so a draw that returned the old one would not have satisfied it.
fn fresh_tiebreaker(previous: u64) -> u64 {
    let mut next: u64 = rand::random();
    while next == previous {
        next = rand::random();
    }
    next
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

    const ALICE: &str = "192.0.2.1:5000";
    const BOB: &str = "192.0.2.2:5000";

    fn address(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    fn credentials(ufrag: &str) -> Credentials {
        Credentials::new(ufrag, format!("{ufrag}xxxxxxxxxxxxxxxxxxxxxxxx")).unwrap()
    }

    fn host(address: SocketAddr) -> Gathered {
        Gathered {
            base: LocalBase(0),
            base_address: address,
            address,
            kind: CandidateType::Host,
            component: ComponentId::RTP,
            server: None,
        }
    }

    fn host_line(address: SocketAddr, foundation: &str) -> Candidate {
        Candidate::parse(&format!(
            "{foundation} 1 UDP 2130706431 {} {} typ host",
            address.ip(),
            address.port()
        ))
        .unwrap()
    }

    /// Two agents wired to each other, each believing it sent the initial offer and each holding
    /// `tiebreaker` — which is §7.3.1.1's `T = V` row, the one that decides whether two copies of
    /// the same stack converge.
    fn both_controlling(tiebreaker: u64) -> (Agent, Agent) {
        let (alice, bob) = (address(ALICE), address(BOB));
        let mut a = Agent::new(Config::default(), true, credentials("aaaa"), tiebreaker);
        let mut b = Agent::new(Config::default(), true, credentials("bbbb"), tiebreaker);
        a.handle(Input::LocalCandidate(host(alice)));
        b.handle(Input::LocalCandidate(host(bob)));
        a.handle(Input::RemoteDescription {
            credentials: credentials("bbbb"),
            candidates: vec![host_line(bob, "1")],
            lite: false,
        });
        b.handle(Input::RemoteDescription {
            credentials: credentials("aaaa"),
            candidates: vec![host_line(alice, "1")],
            lite: false,
        });
        a.handle(Input::GatheringDone);
        b.handle(Input::GatheringDone);
        (a, b)
    }

    /// Run `rounds` Ta ticks at both ends, carrying every datagram one produces to the other and
    /// following the exchange until it goes quiet. No clock, no socket: the "network" is this
    /// function, which is the point of the sans-IO shape.
    fn exchange(a: &mut Agent, b: &mut Agent, rounds: usize) {
        let (alice, bob) = (address(ALICE), address(BOB));
        for _ in 0..rounds {
            let mut pending: Vec<(bool, Vec<u8>)> = Vec::new();
            for output in a.handle(Input::TimerFired(Timer::Ta)) {
                if let Output::Send { bytes, .. } = output {
                    pending.push((true, bytes));
                }
            }
            for output in b.handle(Input::TimerFired(Timer::Ta)) {
                if let Output::Send { bytes, .. } = output {
                    pending.push((false, bytes));
                }
            }
            for _ in 0..8 {
                let mut next: Vec<(bool, Vec<u8>)> = Vec::new();
                for (to_bob, bytes) in pending {
                    let (target, from) = if to_bob { (&mut *b, alice) } else { (&mut *a, bob) };
                    for output in target.handle(Input::Datagram {
                        from,
                        on: LocalBase(0),
                        bytes,
                    }) {
                        if let Output::Send { bytes, .. } = output {
                            next.push((!to_bob, bytes));
                        }
                    }
                }
                if next.is_empty() {
                    break;
                }
                pending = next;
            }
        }
    }

    /// The failing-first test of this story, and the one §7.1's note exists for: two agents can
    /// both believe they offered — third-party call control, glare, a re-INVITE crossing — and two
    /// controlling agents never converge, because neither will accept the other's nomination.
    ///
    /// They are given the *same* tiebreaker, so this is §7.3.1.1's `T = V` row at both ends
    /// simultaneously: each 487s the other, each switches under §7.2.5.1, and only the fresh
    /// tiebreakers §7.2.5.1 mandates break the symmetry on the second round.
    #[test]
    fn two_agents_that_both_start_controlling_converge_on_one_role() {
        let (mut alice, mut bob) = both_controlling(0x1234_5678_9abc_def0);
        assert_eq!(alice.role(), Role::Controlling);
        assert_eq!(bob.role(), Role::Controlling);

        exchange(&mut alice, &mut bob, 6);

        assert_ne!(
            alice.role(),
            bob.role(),
            "two controlling agents never converge: neither accepts the other's nomination"
        );
        assert!(alice.role().is_controlling() || bob.role().is_controlling());
    }
}
