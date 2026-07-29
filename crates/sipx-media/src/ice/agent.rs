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
    CandidateIds, Foundations, Gathered, LocalBase, LocalCandidate, LocalId, PairFoundation,
    RemoteCandidate, RemoteId, assign_local_preferences, find_local, find_remote,
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

/// How far the agent has got towards having checklists to pace.
///
/// An ordered three-state and not a pair of flags: "gathering finished" and "the checklists are
/// formed" are not independent, and the states they can be in together are exactly these three.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    /// The driver is still gathering candidates.
    Gathering,
    /// Gathering is finished. The checklists form as soon as both halves of the exchange are in.
    Gathered,
    /// The checklists exist and Ta is pacing over them.
    Checking,
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
    candidate_ids: CandidateIds,
    /// Whether a Ta timer is outstanding, so that a path which creates work after the checklist
    /// set went quiet can arm one and a path that did not cannot arm two.
    ///
    /// Without it, §7.3.1.4's triggered check for an address first seen *after* a checklist
    /// completed is enqueued and never sent: `conclude` cleared Ta and only a Ta tick sends
    /// anything. That is §8.1.1's tolerance clause dead on arrival, and it is not observable in a
    /// test that fires Ta by hand.
    ta_armed: bool,
    set: ChecklistSet,
    transactions: Vec<Transaction>,
    phase: Phase,
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
    /// two full agents; `tiebreaker` is §7.1.3's 64-bit value, chosen at random per ICE session.
    ///
    /// The *initial* tiebreaker is the caller's so that a test can choose it — §7.3.1.1's `T = V`
    /// row is not reachable otherwise. The redraw after a 487 is not, and cannot be: §7.2.5.1
    /// requires the value to change, and in the symmetric conflict both ends apply the same rule
    /// to the same old value, so anything derived from it leaves them equal and oscillating (see
    /// this module's `fresh_tiebreaker`). That one path reaches for the process RNG, as
    /// [`stun::new_transaction_id`] already does for every check's transaction ID. Randomness is
    /// not I/O — no clock is read and no socket is touched — but it does mean the 487 path is the
    /// one place a test can pin only that the value *changed*, not what it became.
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
            candidate_ids: CandidateIds::default(),
            ta_armed: false,
            set: ChecklistSet::new(),
            transactions: Vec::new(),
            phase: Phase::Gathering,
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
                self.phase = self.phase.max(Phase::Gathered);
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

    /// A description from the peer — the first one, a later one for the same ICE session, or an
    /// ICE restart.
    ///
    /// The candidate list is **merged**, never replaced. Every live [`CandidatePair`] holds a
    /// [`RemoteId`], and RFC 8839 §4.2 lets a peer send more than one description for the same
    /// session — a 183 with SDP and then a 200 with SDP, or any re-INVITE. Replacing the table
    /// under the pairs would leave each of them naming a candidate it was never formed for, or
    /// naming nothing at all, and an agent whose every pair dangles sends no checks, reports no
    /// failure and is simply silent. The candidate list is the peer's to choose, so that is a
    /// re-offer silencing ICE.
    ///
    /// Changing **both** `ice-ufrag` and `ice-pwd` is RFC 8839 §4.4.1.1.1's ICE restart, and only
    /// that rebuilds: new checklists, new pair states, nothing carried over but the selected pair
    /// media is still flowing on ([spec] §13.2).
    ///
    /// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
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
        let restart = self.peering.as_ref().is_some_and(|peering| {
            let known = peering.remote();
            known.ufrag() != credentials.ufrag() && known.pwd() != credentials.pwd()
        });
        self.peering = Some(Peering::new(self.credentials.clone(), credentials));
        if restart {
            self.restart(out);
        }
        let added = self.merge_remote_candidates(candidates);
        if self.phase == Phase::Checking {
            if added > 0 {
                self.extend(out);
            }
        } else {
            self.start(out);
        }
    }

    /// Add the candidates this description brought that we do not already have, by address.
    ///
    /// Returns how many were new. A candidate we learned as peer-reflexive (§7.3.1.3) and the
    /// peer has now signalled properly keeps its identity and its pairs; §7.3.1.3 says as much —
    /// "if any subsequent candidate exchanges contain this peer-reflexive candidate, it will
    /// signal the actual foundation for the candidate".
    fn merge_remote_candidates(&mut self, candidates: &[Candidate]) -> usize {
        let mut added = 0usize;
        for candidate in candidates {
            let Some(parsed) = RemoteCandidate::signalled(RemoteId(0), candidate) else {
                continue;
            };
            if let Some(known) = self
                .remote
                .iter_mut()
                .find(|known| known.address == parsed.address)
            {
                // Keep the identity — pairs hold it — and take the description's word for the
                // rest.
                known.foundation = parsed.foundation;
                known.kind = parsed.kind;
                known.priority = parsed.priority;
                continue;
            }
            let id = self.candidate_ids.remote();
            self.remote.push(RemoteCandidate { id, ..parsed });
            added = added.saturating_add(1);
        }
        added
    }

    /// RFC 8839 §4.4.1.1.1: everything is rebuilt for the new ICE session.
    fn restart(&mut self, out: &mut Vec<Output>) {
        for transaction in std::mem::take(&mut self.transactions) {
            out.push(Output::ClearTimer(Timer::Retransmit(transaction.pair)));
        }
        self.remote.clear();
        self.set = ChecklistSet::new();
        self.nominating.clear();
        self.nominate_on_success.clear();
        self.stopping = Stopping::Idle;
        self.failed.clear();
        self.phase = self.phase.min(Phase::Gathered);
        if self.ta_armed {
            self.ta_armed = false;
            out.push(Output::ClearTimer(Timer::Ta));
        }
        // `selected` is deliberately kept: media keeps flowing on the old selected pair until the
        // new session selects one ([spec] §13.2), and the keepalive timer with it.
    }

    /// §6.1.2: "If candidates are added to a checklist … the agent will re-perform these steps for
    /// the updated checklist."
    ///
    /// The pairs already in the set keep their state and their identities; only the new ones are
    /// formed, pruned against what is there, limited and then unfrozen.
    fn extend(&mut self, out: &mut Vec<Output>) {
        let paired: Vec<RemoteId> = self
            .set
            .checklists()
            .iter()
            .flat_map(|list| list.pairs().iter().map(|pair| pair.remote))
            .collect();
        let fresh: Vec<RemoteCandidate> = self
            .remote
            .iter()
            .filter(|candidate| !paired.contains(&candidate.id))
            .cloned()
            .collect();
        if fresh.is_empty() {
            return;
        }
        let pairs = form_pairs(&mut self.ids, self.role, &self.local, &fresh);
        for pair in pairs {
            self.insert_pair(pair);
        }
        self.set.limit(self.config.pair_limit);
        self.forget_unreferenced_remotes();
        self.set.unfreeze_added();
        self.arm_ta(out);
    }

    fn local_candidate(&mut self, gathered: Gathered) {
        let foundation = self.foundations.assign(&gathered, TRANSPORT);
        self.local.push(LocalCandidate {
            id: self.candidate_ids.local(),
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
        if self.phase != Phase::Gathered || self.peering.is_none() {
            return;
        }
        if self.local.is_empty() || self.remote.is_empty() {
            return;
        }
        self.phase = Phase::Checking;
        let pairs = form_pairs(&mut self.ids, self.role, &self.local, &self.remote);
        self.set = ChecklistSet::new();
        self.set.push(Checklist::new(pairs));
        self.set.limit(self.config.pair_limit);
        self.set.compute_initial_states();
        self.arm_ta(out);
    }

    /// Arm Ta if it is not already armed.
    ///
    /// Every path that creates work goes through here, not only [`Agent::start`]: a triggered
    /// check enqueued after a checklist completed has to be able to restart the pacing, or it is
    /// queued for a tick that will never come.
    fn arm_ta(&mut self, out: &mut Vec<Output>) {
        if self.ta_armed {
            return;
        }
        self.ta_armed = true;
        out.push(Output::SetTimer {
            timer: Timer::Ta,
            after: self.config.timers.pacing(),
        });
    }

    /// Insert a pair, unless the checklist already holds one that §6.1.2.4 would call redundant
    /// with it — the same local base against the same remote address.
    fn insert_pair(&mut self, pair: CandidatePair) {
        let Some((base, remote)) = find_local(&self.local, pair.local)
            .map(|local| local.gathered.base_address)
            .zip(find_remote(&self.remote, pair.remote).map(|remote| remote.address))
        else {
            return;
        };
        let redundant = self.set.checklists().iter().any(|list| {
            list.pairs().iter().any(|known| {
                find_local(&self.local, known.local)
                    .is_some_and(|local| local.gathered.base_address == base)
                    && find_remote(&self.remote, known.remote)
                        .is_some_and(|known| known.address == remote)
            })
        });
        if redundant {
            return;
        }
        if let Some(list) = self.set.checklists_mut().first_mut() {
            list.insert(pair);
        }
    }

    /// Drop peer-reflexive remote candidates that no pair and no valid pair refers to any more.
    ///
    /// §6.1.2.5's limit bounds the pairs; this bounds the table they index. Without it, §7.3.1.3
    /// grows one remote candidate per distinct source address that can produce an authenticated
    /// check, which is unbounded even when every pair it would have formed was discarded.
    fn forget_unreferenced_remotes(&mut self) {
        let mut live: Vec<RemoteId> = self
            .set
            .checklists()
            .iter()
            .flat_map(|list| list.pairs().iter().map(|pair| pair.remote))
            .collect();
        let addresses: Vec<SocketAddr> = self
            .set
            .checklists()
            .iter()
            .flat_map(|list| list.valid().iter().map(|valid| valid.remote))
            .chain(self.selected.iter().map(|selection| selection.remote))
            .collect();
        for candidate in &self.remote {
            if addresses.contains(&candidate.address) {
                live.push(candidate.id);
            }
        }
        self.remote.retain(|candidate| {
            candidate.kind != CandidateType::PeerReflexive || live.contains(&candidate.id)
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
        // The timer that brought us here has fired, so it is no longer outstanding.
        self.ta_armed = false;
        let count = self.set.checklists().len();
        for _ in 0..count {
            let Some(index) = self.set.next_active() else {
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
            if self
                .set
                .checklists()
                .get(index)
                .is_some_and(|list| list.state() != ChecklistState::Running)
            {
                // A concluded checklist answers what is still queued for it and starts nothing
                // new (§8.1.2).
                continue;
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
        if self.active() {
            self.arm_ta(out);
        }
    }

    /// Whether any checklist still has work: one in the Running state, or one whose
    /// triggered-check queue is not empty.
    ///
    /// The second half is what keeps §8.1.1's tolerance clause meaningful. A peer that nominates
    /// more than once has its later nominations answered by a checklist that is already
    /// Completed, and a Ta tick that stops at the first Completed checklist would leave those
    /// triggered checks queued forever — so the highest-priority nominated pair would never be
    /// the selected one.
    fn active(&self) -> bool {
        self.set
            .checklists()
            .iter()
            .any(|list| list.state() == ChecklistState::Running || list.has_triggered())
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
        let (Some(local), Some(remote)) = (
            find_local(&self.local, local_id),
            find_remote(&self.remote, remote_id),
        ) else {
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
                    out.push(Output::Send {
                        on,
                        to: from,
                        bytes,
                    });
                }
                return;
            }
            Conflict::Switched | Conflict::None => {}
        }

        // §7.3.1: the rest runs whether or not the role changed, so long as a success response is
        // generated — which it is, from here on.
        if let Ok(bytes) = stun::check_success(message.transaction(), &peering, from) {
            out.push(Output::Send {
                on,
                to: from,
                bytes,
            });
        }
        if self.phase != Phase::Checking {
            // No checklist yet, so there is nothing to trigger. The response above still went, as
            // §7.3 requires of an agent that has published a candidate on this base.
            return;
        }

        let Some(local_id) = self.base_candidate(on) else {
            return;
        };
        let Some(component) =
            find_local(&self.local, local_id).map(|candidate| candidate.gathered.component)
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
        if let Some(known) = self
            .remote
            .iter()
            .find(|candidate| candidate.address == from)
        {
            return known.id;
        }
        let id = self.candidate_ids.remote();
        self.remote.push(RemoteCandidate {
            id,
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
        id
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
            self.insert_pair(pair);
            if let Some(pair) = self.set.pair_mut(id) {
                pair.state = PairState::Waiting;
            }
            // §6.1.2.5's limit binds here and not only at formation. This is the growth path a
            // peer drives: one pair and one remote candidate per distinct source address that can
            // produce an authenticated check, each of which would otherwise become an 88-byte
            // check sent to an address the peer named and need not be able to receive at. The
            // limit is the bound §19.5.1 asks for, so it is applied every time the set grows.
            self.set.limit(self.config.pair_limit);
            self.forget_unreferenced_remotes();
            if self.set.pair(id).is_none() {
                // The new pair was the lowest-priority discardable one: the set is full of better
                // paths. The check that arrived is still answered, above; it just does not earn a
                // check of its own.
                return;
            }
            if let Some(index) = self.set.checklist_of(id)
                && let Some(list) = self.set.checklists_mut().get_mut(index)
            {
                list.trigger(id);
            }
            self.arm_ta(out);
            id
        };
        let before = self.set.pair(id).map(|pair| pair.state);
        match before {
            Some(PairState::Succeeded) => {
                // "If the state of that pair is Succeeded, nothing further is done."
            }
            Some(_) if self.set.pair(id).is_some_and(|pair| pair.nominated) => {
                // §8.1.2: "when the state of a pair is Succeeded, an agent will no longer
                // generate triggered checks when receiving a Binding request for the pair."
                //
                // It has to extend past Succeeded to a nominated pair in *any* state, or two
                // concluded agents re-trigger each other forever: a queued check of our own moves
                // the pair out of Succeeded, the peer's next request then finds it In-Progress
                // and §7.3.1.4's cancel-and-re-enqueue fires, and each end keeps the other's
                // queue full. Media flows on the selected pair the whole time, so the traffic is
                // pure waste — and the checklist never falls quiet, which is what a driver waits
                // for.
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
                // §7.3.1.4's check has to be able to leave even when the checklist that holds it
                // has already concluded — see [`Agent::ta_armed`].
                self.arm_ta(out);
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
        let local_candidate = find_local(&self.local, local)?;
        let remote_candidate = find_remote(&self.remote, remote)?;
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
            .find(|candidate| {
                candidate.gathered.base == on && candidate.gathered.kind == CandidateType::Host
            })
            .map(|candidate| candidate.id)
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
        let priority = find_local(&self.local, local)
            .zip(find_remote(&self.remote, pair.remote))
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
    fn learn_local(
        &mut self,
        mapped: SocketAddr,
        pair: &CandidatePair,
        claimed: Priority,
    ) -> LocalId {
        if let Some(known) = self
            .local
            .iter()
            .find(|candidate| candidate.gathered.address == mapped)
        {
            return known.id;
        }
        let Some(base) = find_local(&self.local, pair.local).copied() else {
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
        let id = self.candidate_ids.local();
        self.local.push(LocalCandidate {
            id,
            gathered,
            foundation,
            local_preference: base.local_preference,
            // "The priority is the value of the PRIORITY attribute in the Binding request" — the
            // one this agent sent, which §7.1.1 already computed with the peer-reflexive
            // preference. That is what makes both ends price this candidate the same.
            priority: claimed,
        });
        id
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
        let theirs = attribute.tiebreaker();
        match (self.role, attribute) {
            (Role::Controlling, RoleAttribute::Controlling { .. }) => {
                if self.tiebreaker >= theirs {
                    Conflict::Reject
                } else {
                    self.switch_role();
                    Conflict::Switched
                }
            }
            (Role::Controlled, RoleAttribute::Controlled { .. }) => {
                if self.tiebreaker >= theirs {
                    self.switch_role();
                    Conflict::Switched
                } else {
                    Conflict::Reject
                }
            }
            // Controlling against ICE-CONTROLLED, or controlled against ICE-CONTROLLING.
            _ => Conflict::None,
        }
    }

    /// §7.2.5.1: our own check drew a 487.
    fn role_conflict_response(&mut self, transaction: &Transaction) {
        // "If the agent included an ICE-CONTROLLED attribute in the request, the agent MUST switch
        // to the controlling role. If the agent included an ICE-CONTROLLING attribute … switch to
        // the controlled role." The attribute that went out decides, not the role we hold now.
        self.role = match transaction.role {
            RoleAttribute::Controlled { .. } => Role::Controlling,
            RoleAttribute::Controlling { .. } => Role::Controlled,
        };
        // "The agent MUST change the tiebreaker value."
        self.tiebreaker = fresh_tiebreaker(self.tiebreaker);
        self.set
            .recompute_priorities(self.role, &self.local, &self.remote);
        if let Some(pair) = self.set.pair_mut(transaction.pair) {
            pair.state = PairState::Waiting;
        }
        if let Some(index) = self.set.checklist_of(transaction.pair)
            && let Some(list) = self.set.checklists_mut().get_mut(index)
        {
            list.trigger(transaction.pair);
        }
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
    /// **On a large checklist set only `Tn` fires this, and that is by design.** §14.3 scales the
    /// RTO with `Ta × N × (Num-Waiting + Num-In-Progress)`, so a set at §6.1.2.5's default limit
    /// of 100 pairs starts every transaction at `50 ms × 100 × 100` = **500 s**, and Rc = 7
    /// transmissions with the doubling and the Rm final wait take over ten hours to exhaust. That
    /// is one transmission per pair per call: a higher-priority pair that simply gets no answer
    /// never reaches `Failed` inside any real session, so the "every pair of higher priority than
    /// the best valid pair has reached `Failed`" half of the criterion cannot become true and the
    /// whole decision rests on `Tn`. §19.5.1 treats that pacing as intended — it is the bound on
    /// what a candidate list can cost — so `Tn` is the criterion for any set of interesting size
    /// and the `Failed` half is the fast path for a small one, not the other way round.
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
            let Some(base) =
                find_local(&self.local, *local).map(|candidate| candidate.gathered.base)
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
            // §8.1.2's pruning, for the agent that knows there will be no second nomination.
            // A controlled agent must not do it: §8.1.1 requires it to tolerate a peer that
            // nominates more than once and then "use the pairs with the highest priority", and a
            // checklist it has already emptied has nothing left to raise the selection to.
            if self.role.is_controlling() {
                if let Some(list) = self.set.checklists_mut().get_mut(index) {
                    list.keep_only_nominated(*component, *pair);
                }
                // §8.1.2: "if the state of a pair is In-Progress, the agent cancels the
                // In-Progress transaction". A removed pair leaves a transaction behind that
                // `retransmit` would happily keep servicing — invisible with one component,
                // because the last of them is cleared below, and a live retransmission loop for a
                // pair that no longer exists as soon as there are two.
                let live: Vec<PairId> = self
                    .set
                    .checklists()
                    .iter()
                    .flat_map(|list| list.pairs().iter().map(|pair| pair.id))
                    .collect();
                for transaction in &self.transactions {
                    if !live.contains(&transaction.pair) {
                        out.push(Output::ClearTimer(Timer::Retransmit(transaction.pair)));
                    }
                }
                self.transactions
                    .retain(|transaction| live.contains(&transaction.pair));
            }
        }

        if chosen.len() == components.len() && !components.is_empty() {
            if let Some(list) = self.set.checklists_mut().get_mut(index) {
                list.set_state(ChecklistState::Completed);
            }
            if !self.active() {
                for transaction in std::mem::take(&mut self.transactions) {
                    out.push(Output::ClearTimer(Timer::Retransmit(transaction.pair)));
                }
                self.ta_armed = false;
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
            // A priority of its own, so that a pair's `G` and `D` differ and §6.1.2.3's
            // recomputation on a role change is visible.
            "{foundation} 1 UDP 1694498815 {} {} typ host",
            address.ip(),
            address.port()
        ))
        .unwrap()
    }

    /// What a driver does with Ta: fire it when, and only when, the agent has armed one.
    ///
    /// A test that fires Ta by hand cannot tell an armed timer from an invented one, and that is
    /// exactly the gap this type closes — a triggered check enqueued after a checklist concluded
    /// is queued for a tick a real driver would never deliver.
    #[derive(Debug, Default)]
    struct Driver {
        armed: bool,
    }

    impl Driver {
        /// Deliver the Ta tick the agent asked for. A one-shot timer that has fired is no longer
        /// armed, so only the agent's own `SetTimer` can arm the next one.
        fn tick(&mut self, agent: &mut Agent) -> Vec<Output> {
            self.armed = false;
            let outputs = agent.handle(Input::TimerFired(Timer::Ta));
            self.absorb(&outputs);
            outputs
        }

        fn absorb(&mut self, outputs: &[Output]) {
            for output in outputs {
                match output {
                    Output::SetTimer {
                        timer: Timer::Ta, ..
                    } => self.armed = true,
                    Output::ClearTimer(Timer::Ta) => self.armed = false,
                    _ => {}
                }
            }
        }
    }

    fn two_agents(
        offerer: (bool, bool),
        tiebreakers: (u64, u64),
    ) -> (Agent, Agent, Driver, Driver) {
        let (alice_address, bob_address) = (address(ALICE), address(BOB));
        let mut alice = Agent::new(
            Config::default(),
            offerer.0,
            credentials("aaaa"),
            tiebreakers.0,
        );
        let mut bob = Agent::new(
            Config::default(),
            offerer.1,
            credentials("bbbb"),
            tiebreakers.1,
        );
        let (mut left, mut right) = (Driver::default(), Driver::default());
        left.absorb(&alice.handle(Input::LocalCandidate(host(alice_address))));
        right.absorb(&bob.handle(Input::LocalCandidate(host(bob_address))));
        left.absorb(&alice.handle(Input::RemoteDescription {
            credentials: credentials("bbbb"),
            candidates: vec![host_line(bob_address, "1")],
            lite: false,
        }));
        right.absorb(&bob.handle(Input::RemoteDescription {
            credentials: credentials("aaaa"),
            candidates: vec![host_line(alice_address, "1")],
            lite: false,
        }));
        left.absorb(&alice.handle(Input::GatheringDone));
        right.absorb(&bob.handle(Input::GatheringDone));
        (alice, bob, left, right)
    }

    /// Two agents wired to each other, each believing it sent the initial offer and each holding
    /// `tiebreaker` — which is §7.3.1.1's `T = V` row, the one that decides whether two copies of
    /// the same stack converge.
    fn both_controlling(tiebreaker: u64) -> (Agent, Agent, Driver, Driver) {
        two_agents((true, true), (tiebreaker, tiebreaker))
    }

    /// Run `rounds` Ta ticks at both ends — but only at an end that has a Ta armed — carrying
    /// every datagram one produces to the other and following the exchange until it goes quiet.
    /// No clock, no socket: the "network" is this function, which is the point of the sans-IO
    /// shape.
    fn exchange(
        a: &mut Agent,
        b: &mut Agent,
        left: &mut Driver,
        right: &mut Driver,
        rounds: usize,
    ) {
        let (alice, bob) = (address(ALICE), address(BOB));
        for _ in 0..rounds {
            let mut pending: Vec<(bool, Vec<u8>)> = Vec::new();
            if left.armed {
                for output in left.tick(a) {
                    if let Output::Send { bytes, .. } = output {
                        pending.push((true, bytes));
                    }
                }
            }
            if right.armed {
                for output in right.tick(b) {
                    if let Output::Send { bytes, .. } = output {
                        pending.push((false, bytes));
                    }
                }
            }
            for _ in 0..8 {
                let mut next: Vec<(bool, Vec<u8>)> = Vec::new();
                for (to_bob, bytes) in pending {
                    let (target, driver, from) = if to_bob {
                        (&mut *b, &mut *right, alice)
                    } else {
                        (&mut *a, &mut *left, bob)
                    };
                    let outputs = target.handle(Input::Datagram {
                        from,
                        on: LocalBase(0),
                        bytes,
                    });
                    driver.absorb(&outputs);
                    for output in outputs {
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

    fn agent(offerer: bool, tiebreaker: u64, remotes: &[SocketAddr]) -> Agent {
        let mut agent = Agent::new(Config::default(), offerer, credentials("aaaa"), tiebreaker);
        agent.handle(Input::LocalCandidate(host(address(ALICE))));
        agent.handle(Input::RemoteDescription {
            credentials: credentials("bbbb"),
            candidates: remotes
                .iter()
                .enumerate()
                .map(|(index, remote)| host_line(*remote, &(index + 1).to_string()))
                .collect(),
            lite: false,
        });
        agent.handle(Input::GatheringDone);
        agent
    }

    /// The peer's view of the credential pair. Its `outbound_*` is what a check arriving at our
    /// agent must carry, which is the direction rule `Peering` exists to keep straight.
    fn peer() -> Peering {
        Peering::new(credentials("bbbb"), credentials("aaaa"))
    }

    fn sent(outputs: &[Output]) -> Vec<Message> {
        outputs
            .iter()
            .filter_map(|output| match output {
                Output::Send { bytes, .. } => Message::decode(bytes).ok(),
                _ => None,
            })
            .collect()
    }

    fn requests(outputs: &[Output]) -> Vec<Message> {
        sent(outputs)
            .into_iter()
            .filter(|message| message.class() == Class::Request)
            .collect()
    }

    fn retransmit_after(outputs: &[Output]) -> Option<Duration> {
        outputs.iter().find_map(|output| match output {
            Output::SetTimer {
                timer: Timer::Retransmit(_),
                after,
            } => Some(*after),
            _ => None,
        })
    }

    /// A check from the peer, with whatever role attribute the row under test needs.
    fn peer_check(role: RoleAttribute) -> Vec<u8> {
        stun::connectivity_check(
            stun::new_transaction_id(),
            &peer(),
            Priority::new(1_862_270_975).unwrap(),
            role,
        )
        .unwrap()
    }

    fn deliver(agent: &mut Agent, from: SocketAddr, bytes: Vec<u8>) -> Vec<Output> {
        agent.handle(Input::Datagram {
            from,
            on: LocalBase(0),
            bytes,
        })
    }

    // --------------------------------------------------------------------------- sans-IO

    /// [spec] §2 and the working agreement: no runtime, no socket, no clock read. Asserted on the
    /// source rather than on behaviour, because the failure mode is a single `use` line that
    /// nothing else in this crate would notice — `sipx-media` legitimately depends on `tokio` for
    /// the driver and the session, so a compile-time barrier is not available here.
    ///
    /// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
    #[test]
    fn the_agent_reads_no_clock_and_owns_no_socket() {
        // Comments are stripped — these modules explain the constraint in the words the
        // constraint forbids — but everything else is scanned, including anything below the test
        // module. A scan that stopped at the first `#[cfg(test)]` would not have looked at
        // library code written after it.
        let code = |source: &str| -> String {
            source
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        // Spelled in fragments so that this list is not itself a match for the scan it drives.
        let forbidden = [
            ["tok", "io"].concat(),
            ["Udp", "Socket"].concat(),
            ["Ins", "tant"].concat(),
            ["System", "Time"].concat(),
            ["std::", "thread"].concat(),
        ];
        for source in [
            code(include_str!("agent.rs")),
            code(include_str!("checklist.rs")),
            code(include_str!("candidate.rs")),
            code(include_str!("timing.rs")),
        ] {
            for forbidden in &forbidden {
                assert!(
                    !source.contains(forbidden),
                    "the ICE agent must not reach for {forbidden}: time arrives as TimerFired \
                     and datagrams arrive as bytes"
                );
            }
        }
    }

    // ------------------------------------------------------------------- §7.3.1.1, row by row

    /// [spec] §7.3's table, all seven rows, each its own assertion.
    ///
    /// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
    #[test]
    fn the_role_conflict_table_is_walked_row_by_row() {
        let controlling = |tiebreaker: u64| RoleAttribute::Controlling {
            tiebreaker,
            nominate: false,
        };
        let controlled = |tiebreaker: u64| RoleAttribute::Controlled { tiebreaker };

        // Row 1: controlling, ICE-CONTROLLING, T > V — 487 and keep controlling.
        let mut subject = agent(true, 100, &[address(BOB)]);
        assert_eq!(
            subject.resolve_conflict(Some(controlling(50))),
            Conflict::Reject
        );
        assert_eq!(subject.role(), Role::Controlling);

        // Row 1 again at T = V. `>=` and not `>` is the whole point: with equal tiebreakers
        // neither side may switch on the request, or they simply swap roles.
        let mut subject = agent(true, 100, &[address(BOB)]);
        assert_eq!(
            subject.resolve_conflict(Some(controlling(100))),
            Conflict::Reject
        );
        assert_eq!(subject.role(), Role::Controlling);

        // Row 2: controlling, ICE-CONTROLLING, T < V — switch to controlled, answer normally.
        let mut subject = agent(true, 100, &[address(BOB)]);
        assert_eq!(
            subject.resolve_conflict(Some(controlling(200))),
            Conflict::Switched
        );
        assert_eq!(subject.role(), Role::Controlled);

        // Row 3: controlled, ICE-CONTROLLED, T >= V — switch to controlling, answer normally.
        let mut subject = agent(false, 100, &[address(BOB)]);
        assert_eq!(
            subject.resolve_conflict(Some(controlled(100))),
            Conflict::Switched
        );
        assert_eq!(subject.role(), Role::Controlling);

        // Row 4: controlled, ICE-CONTROLLED, T < V — 487 and keep controlled.
        let mut subject = agent(false, 100, &[address(BOB)]);
        assert_eq!(
            subject.resolve_conflict(Some(controlled(200))),
            Conflict::Reject
        );
        assert_eq!(subject.role(), Role::Controlled);

        // Row 5: controlling, ICE-CONTROLLED — no conflict.
        let mut subject = agent(true, 100, &[address(BOB)]);
        assert_eq!(
            subject.resolve_conflict(Some(controlled(200))),
            Conflict::None
        );
        assert_eq!(subject.role(), Role::Controlling);

        // Row 6: controlled, ICE-CONTROLLING — no conflict.
        let mut subject = agent(false, 100, &[address(BOB)]);
        assert_eq!(
            subject.resolve_conflict(Some(controlling(200))),
            Conflict::None
        );
        assert_eq!(subject.role(), Role::Controlled);

        // Row 7: neither attribute — no conflict; the peer is not doing role signalling.
        let mut subject = agent(true, 100, &[address(BOB)]);
        assert_eq!(subject.resolve_conflict(None), Conflict::None);
        assert_eq!(subject.role(), Role::Controlling);
    }

    /// The rejecting rows put a 487 on the wire and answer nothing else.
    #[test]
    fn a_rejected_role_conflict_answers_487_and_not_a_success() {
        let mut subject = agent(true, 100, &[address(BOB)]);
        let outputs = deliver(
            &mut subject,
            address(BOB),
            peer_check(RoleAttribute::Controlling {
                tiebreaker: 100,
                nominate: false,
            }),
        );
        let answers = sent(&outputs);
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].class(), Class::Error);
        assert_eq!(answers[0].error_code(), Some(stun::ROLE_CONFLICT));
        assert_eq!(subject.role(), Role::Controlling);
    }

    /// …and the switching rows answer normally, because §7.3.1's remaining processing "[is]
    /// followed if the agent generated a successful response, even if the agent changed roles".
    #[test]
    fn a_switched_role_still_answers_the_check_that_caused_it() {
        let mut subject = agent(true, 100, &[address(BOB)]);
        let outputs = deliver(
            &mut subject,
            address(BOB),
            peer_check(RoleAttribute::Controlling {
                tiebreaker: 200,
                nominate: false,
            }),
        );
        let answers = sent(&outputs);
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].class(), Class::Success);
        assert_eq!(subject.role(), Role::Controlled);
    }

    /// §7.2.5.1, every clause of it: switch to the role opposite the attribute that went out,
    /// change the tiebreaker, recompute every pair priority, and re-run the check as a triggered
    /// one so the new role goes out immediately.
    #[test]
    fn a_487_switches_the_role_changes_the_tiebreaker_and_re_runs_the_check() {
        let mut subject = agent(true, 100, &[address(BOB)]);
        let check = subject.handle(Input::TimerFired(Timer::Ta));
        let outgoing = requests(&check);
        assert_eq!(outgoing.len(), 1);
        let transaction = outgoing[0].transaction();
        let before = subject.checklists().checklists()[0].pairs()[0].priority;
        let pair = subject.checklists().checklists()[0].pairs()[0].id;
        assert_eq!(
            subject.checklists().pair(pair).unwrap().state,
            PairState::InProgress
        );

        let rejection = stun::role_conflict(transaction, &peer()).unwrap();
        subject.handle(Input::Datagram {
            from: address(BOB),
            on: LocalBase(0),
            bytes: rejection,
        });

        assert_eq!(subject.role(), Role::Controlled);
        assert_ne!(subject.tiebreaker(), 100);
        assert_ne!(
            subject.checklists().checklists()[0].pairs()[0].priority,
            before,
            "a role switch swaps G and D, so every pair priority moves"
        );
        assert_eq!(
            subject.checklists().pair(pair).unwrap().state,
            PairState::Waiting
        );
        assert!(subject.checklists().checklists()[0].is_triggered(pair));

        // And the re-run carries the new role.
        let rerun = subject.handle(Input::TimerFired(Timer::Ta));
        let rerun = requests(&rerun);
        assert_eq!(rerun.len(), 1);
        assert!(matches!(
            rerun[0].role(),
            Some(RoleAttribute::Controlled { .. })
        ));
    }

    // ------------------------------------------------------------------------------- §7.1.1

    /// §7.1.1: the `PRIORITY` in a check is the candidate's priority recomputed with the
    /// peer-reflexive type preference. Get this wrong and the peer prices the peer-reflexive
    /// candidate it learns from this very check differently from us.
    #[test]
    fn a_check_carries_the_peer_reflexive_priority_not_the_candidates_own() {
        let mut subject = agent(true, 100, &[address(BOB)]);
        let outputs = subject.handle(Input::TimerFired(Timer::Ta));
        let check = &requests(&outputs)[0];
        let candidate = subject.local_candidates()[0];
        assert_eq!(candidate.priority.get(), 2_130_706_431);
        assert_eq!(check.priority(), Some(candidate.check_priority()));
        assert_eq!(check.priority().unwrap().get(), 1_862_270_975);
        assert_ne!(check.priority(), Some(candidate.priority));
    }

    // -------------------------------------------------------------- peer-reflexive candidates

    /// §7.3.1.3: a check from an address no `a=candidate` named is a peer-reflexive *remote*
    /// candidate, priced from the `PRIORITY` the check carried.
    #[test]
    fn a_check_from_an_unknown_address_teaches_a_remote_candidate() {
        let mut subject = agent(true, 100, &[address(BOB)]);
        assert_eq!(subject.remote_candidates().len(), 1);
        let behind_a_nat = address("198.51.100.7:41234");
        deliver(
            &mut subject,
            behind_a_nat,
            peer_check(RoleAttribute::Controlled { tiebreaker: 1 }),
        );
        let learned = subject
            .remote_candidates()
            .iter()
            .find(|candidate| candidate.address == behind_a_nat)
            .expect("§7.3.1.3 learns the source of an unmatched check");
        assert_eq!(learned.kind, CandidateType::PeerReflexive);
        assert_eq!(learned.priority.get(), 1_862_270_975);
        assert_eq!(learned.component, ComponentId::RTP);
    }

    /// §7.2.5.3.1: a mapped address that is not one of our local candidates is a peer-reflexive
    /// *local* candidate, and its priority is the `PRIORITY` we put in the request — not
    /// something recomputed, or the two ends disagree.
    #[test]
    fn a_mapped_address_we_do_not_have_teaches_a_local_candidate() {
        let mut subject = agent(true, 100, &[address(BOB)]);
        let outputs = subject.handle(Input::TimerFired(Timer::Ta));
        let transaction = requests(&outputs)[0].transaction();
        let reflexive = address("198.51.100.4:33445");
        let response = stun::check_success(transaction, &peer(), reflexive).unwrap();
        deliver(&mut subject, address(BOB), response);

        let learned = subject
            .local_candidates()
            .iter()
            .find(|candidate| candidate.gathered.address == reflexive)
            .expect("§7.2.5.3.1 learns the mapped address");
        assert_eq!(learned.gathered.kind, CandidateType::PeerReflexive);
        assert_eq!(learned.priority.get(), 1_862_270_975);
        assert_eq!(learned.gathered.base_address, address(ALICE));
    }

    // ---------------------------------------------------------------------- triggered checks

    /// §7.3.1.4: a triggered check jumps the queue, whatever the priorities say. The peer's
    /// low-priority path is checked before our own highest-priority `Waiting` pair.
    #[test]
    fn a_triggered_check_preempts_the_highest_priority_waiting_pair() {
        let low = address("198.51.100.8:40000");
        let mut subject = agent(true, 100, &[address(BOB), low]);
        // The peer checks us from an address that is not even in the checklist yet.
        let surprise = address("198.51.100.9:41000");
        deliver(
            &mut subject,
            surprise,
            peer_check(RoleAttribute::Controlled { tiebreaker: 1 }),
        );

        let outputs = subject.handle(Input::TimerFired(Timer::Ta));
        let addressed: Vec<SocketAddr> = outputs
            .iter()
            .filter_map(|output| match output {
                Output::Send { to, bytes, .. } if Message::decode(bytes).is_ok() => Some(*to),
                _ => None,
            })
            .collect();
        assert_eq!(
            addressed,
            vec![surprise],
            "the triggered check goes first, ahead of every Waiting pair"
        );
    }

    /// Nothing in the machine is a literal: a deployment that halves Ta gets checks at half the
    /// interval, and one that lowers §6.1.2.5's limit gets a smaller checklist set.
    #[test]
    fn the_timers_and_the_pair_limit_are_the_configured_ones() {
        let config = Config {
            timers: Timers {
                ta: Duration::from_millis(20),
                ..Timers::default()
            },
            pair_limit: 3,
        };
        let remotes: Vec<SocketAddr> = (1..=8)
            .map(|n| address(&format!("198.51.100.{n}:5000")))
            .collect();
        let mut subject = Agent::new(config, true, credentials("aaaa"), 100);
        subject.handle(Input::LocalCandidate(host(address(ALICE))));
        subject.handle(Input::RemoteDescription {
            credentials: credentials("bbbb"),
            candidates: remotes
                .iter()
                .enumerate()
                .map(|(index, remote)| host_line(*remote, &(index + 1).to_string()))
                .collect(),
            lite: false,
        });
        let started = subject.handle(Input::GatheringDone);

        assert_eq!(subject.checklists().total_pairs(), 3);
        assert!(started.contains(&Output::SetTimer {
            timer: Timer::Ta,
            after: Duration::from_millis(20),
        }));

        let tick = subject.handle(Input::TimerFired(Timer::Ta));
        assert!(tick.contains(&Output::SetTimer {
            timer: Timer::Ta,
            after: Duration::from_millis(20),
        }));
    }

    // ------------------------------------------------------------------------- §7.2.5.2.1

    /// §7.2.5.2.1: "the source IP address and port of the response MUST be equal to the
    /// destination … to which the Binding request was sent". A response from anywhere else fails
    /// the pair, however well formed and however well authenticated it is.
    #[test]
    fn a_response_from_the_wrong_address_fails_the_pair() {
        let mut subject = agent(true, 100, &[address(BOB)]);
        let outputs = subject.handle(Input::TimerFired(Timer::Ta));
        let transaction = requests(&outputs)[0].transaction();
        let pair = subject.checklists().checklists()[0].pairs()[0].id;

        let response = stun::check_success(transaction, &peer(), address(ALICE)).unwrap();
        deliver(&mut subject, address("198.51.100.66:5000"), response);

        assert_eq!(
            subject.checklists().pair(pair).unwrap().state,
            PairState::Failed
        );
        assert!(subject.checklists().checklists()[0].valid().is_empty());
    }

    /// …and an unauthenticated response moves nothing at all, not even into Failed — otherwise
    /// anyone who can see a check can fail every pair by answering it ([spec] §11.3).
    ///
    /// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
    #[test]
    fn a_response_with_the_wrong_credential_moves_no_state() {
        let mut subject = agent(true, 100, &[address(BOB)]);
        let outputs = subject.handle(Input::TimerFired(Timer::Ta));
        let transaction = requests(&outputs)[0].transaction();
        let pair = subject.checklists().checklists()[0].pairs()[0].id;

        // Keyed with a password that is not ours: `check_success` keys a response with the
        // responder's own credential, which is what our `outbound_key` has to match.
        let forged = Peering::new(credentials("zzzz"), credentials("aaaa"));
        let response = stun::check_success(transaction, &forged, address(ALICE)).unwrap();
        let answered = deliver(&mut subject, address(BOB), response);

        assert!(answered.is_empty());
        assert_eq!(
            subject.checklists().pair(pair).unwrap().state,
            PairState::InProgress
        );
    }

    // ------------------------------------------------------------------------------- §14.3

    /// §14.3: "the RTO will be different for each transaction as the number of checks in the
    /// Waiting and In-Progress states change", so it is computed when a check is sent.
    #[test]
    fn the_rto_is_recomputed_for_every_transaction() {
        let remotes: Vec<SocketAddr> = (1..=5)
            .map(|n| address(&format!("198.51.100.{n}:5000")))
            .collect();
        // Controlled, so that a success does not immediately start nominating and change what is
        // outstanding for a second reason.
        let mut subject = agent(false, 100, &remotes);

        let first = subject.handle(Input::TimerFired(Timer::Ta));
        let first_rto = retransmit_after(&first).expect("a check arms its retransmission timer");
        let transaction = requests(&first)[0].transaction();
        let destination = match first.first() {
            Some(Output::Send { to, .. }) => *to,
            other => panic!("expected a check, got {other:?}"),
        };

        let response = stun::check_success(transaction, &peer(), address(ALICE)).unwrap();
        deliver(&mut subject, destination, response);

        let second = subject.handle(Input::TimerFired(Timer::Ta));
        let second_rto = retransmit_after(&second).expect("the next check arms its own");
        assert!(
            second_rto < first_rto,
            "one fewer outstanding check must shorten the RTO: {second_rto:?} vs {first_rto:?}"
        );
    }

    /// RFC 5389 §7.2.1: Rc transmissions, doubling, then a final wait of Rm times the RTO, and
    /// only then is the pair Failed.
    #[test]
    fn a_check_is_retransmitted_rc_times_before_the_pair_fails() {
        let mut subject = agent(true, 100, &[address(BOB)]);
        let first = subject.handle(Input::TimerFired(Timer::Ta));
        let pair = subject.checklists().checklists()[0].pairs()[0].id;
        let mut interval = retransmit_after(&first).unwrap();

        // Transmissions 2..=Rc, each after twice the last interval.
        for _ in 1..Config::default().timers.rc {
            let outputs = subject.handle(Input::TimerFired(Timer::Retransmit(pair)));
            assert_eq!(requests(&outputs).len(), 1, "a retransmission is a resend");
            let next = retransmit_after(&outputs).unwrap();
            assert_eq!(next, interval * 2);
            interval = next;
            assert_eq!(
                subject.checklists().pair(pair).unwrap().state,
                PairState::InProgress
            );
        }

        // The final wait: Rm times the transaction's first RTO, and no further request.
        let last = subject.handle(Input::TimerFired(Timer::Retransmit(pair)));
        assert!(requests(&last).is_empty());
        let timers = Config::default().timers;
        assert_eq!(
            retransmit_after(&last),
            Some(timers.final_wait(retransmit_after(&first).unwrap()))
        );
        assert_eq!(
            subject.checklists().pair(pair).unwrap().state,
            PairState::InProgress
        );

        // And now it has timed out (§7.2.5.2.3).
        let done = subject.handle(Input::TimerFired(Timer::Retransmit(pair)));
        assert_eq!(
            subject.checklists().pair(pair).unwrap().state,
            PairState::Failed
        );
        assert!(
            done.iter()
                .any(|output| matches!(output, Output::Failed { .. })),
            "the only pair failed, so the component failed"
        );
    }

    // -------------------------------------------------------------------------- nomination

    /// Two agents that agree on their roles converge on a selected pair for the component, at
    /// both ends, by §8.1.1's regular nomination.
    #[test]
    fn two_agents_converge_on_a_selected_pair() {
        let (alice_address, bob_address) = (address(ALICE), address(BOB));
        let (mut alice, mut bob, mut left, mut right) = two_agents((true, false), (900, 100));
        assert_eq!(alice.role(), Role::Controlling);
        assert_eq!(bob.role(), Role::Controlled);

        exchange(&mut alice, &mut bob, &mut left, &mut right, 6);

        assert_eq!(
            alice.selected(ComponentId::RTP),
            Some((LocalBase(0), bob_address))
        );
        assert_eq!(
            bob.selected(ComponentId::RTP),
            Some((LocalBase(0), alice_address))
        );
        assert_eq!(
            alice.checklists().checklists()[0].state(),
            ChecklistState::Completed
        );
        // And both ends fall quiet. §8.1.2 stops an agent generating triggered checks for a
        // concluded pair, without which each end's redundant check finds the other's pair
        // In-Progress, §7.3.1.4 re-enqueues it, and two agreeing agents check each other for the
        // life of the call.
        assert!(!left.armed && !right.armed);
    }

    /// A §7.3.1.4 check for an address first seen *after* the checklist concluded has to be able
    /// to leave. `conclude` clears Ta and `pace` re-arms only while there is work, so without an
    /// arming of its own the triggered check is enqueued for a tick a driver would never deliver
    /// — and the same silence swallows §8.1.1's tolerance clause below.
    #[test]
    fn a_check_arriving_after_the_checklist_concluded_still_arms_ta() {
        let (mut alice, mut bob, mut left, mut right) = two_agents((true, false), (900, 100));
        // Long enough for both ends to conclude and for the pacing to fall quiet.
        exchange(&mut alice, &mut bob, &mut left, &mut right, 12);
        assert_eq!(
            bob.checklists().checklists()[0].state(),
            ChecklistState::Completed
        );
        assert!(!right.armed, "a concluded checklist stops pacing");

        // The peer checks from an address ICE has never seen — a NAT rebinding, say.
        let surprise = address("198.51.100.77:41000");
        let check = stun::connectivity_check(
            stun::new_transaction_id(),
            &Peering::new(credentials("aaaa"), credentials("bbbb")),
            Priority::new(1_862_270_975).unwrap(),
            RoleAttribute::Controlling {
                tiebreaker: 900,
                nominate: false,
            },
        )
        .unwrap();
        let outputs = bob.handle(Input::Datagram {
            from: surprise,
            on: LocalBase(0),
            bytes: check,
        });
        right.absorb(&outputs);
        assert!(
            right.armed,
            "the triggered check §7.3.1.4 just enqueued needs a Ta tick to leave"
        );

        let tick = right.tick(&mut bob);
        let addressed: Vec<SocketAddr> = tick
            .iter()
            .filter_map(|output| match output {
                Output::Send { to, bytes, .. } if Message::decode(bytes).is_ok() => Some(*to),
                _ => None,
            })
            .collect();
        assert_eq!(addressed, vec![surprise]);
    }

    /// §8.1.1: "the agent MUST NOT nominate another pair for [the] same component … within the
    /// ICE session". One `USE-CANDIDATE` leaves this agent, ever.
    #[test]
    fn the_controlling_agent_nominates_a_component_exactly_once() {
        let (alice_address, bob_address) = (address(ALICE), address(BOB));
        let mut alice = Agent::new(Config::default(), true, credentials("aaaa"), 900);
        let mut bob = Agent::new(Config::default(), false, credentials("bbbb"), 100);
        alice.handle(Input::LocalCandidate(host(alice_address)));
        bob.handle(Input::LocalCandidate(host(bob_address)));
        alice.handle(Input::RemoteDescription {
            credentials: credentials("bbbb"),
            candidates: vec![host_line(bob_address, "1")],
            lite: false,
        });
        bob.handle(Input::RemoteDescription {
            credentials: credentials("aaaa"),
            candidates: vec![host_line(alice_address, "1")],
            lite: false,
        });
        alice.handle(Input::GatheringDone);
        bob.handle(Input::GatheringDone);

        let mut nominations = 0usize;
        for _ in 0..10 {
            let outputs = alice.handle(Input::TimerFired(Timer::Ta));
            for message in requests(&outputs) {
                if message.use_candidate() {
                    nominations += 1;
                }
            }
            for output in outputs {
                if let Output::Send { bytes, .. } = output {
                    for answer in bob.handle(Input::Datagram {
                        from: alice_address,
                        on: LocalBase(0),
                        bytes,
                    }) {
                        if let Output::Send { bytes, .. } = answer {
                            alice.handle(Input::Datagram {
                                from: bob_address,
                                on: LocalBase(0),
                                bytes,
                            });
                        }
                    }
                }
            }
        }
        assert_eq!(nominations, 1, "regular nomination nominates once");
    }

    /// §7.1.2 makes `USE-CANDIDATE` the controlling agent's alone, and the type system makes it
    /// unsendable by a controlled one — [`RoleAttribute::Controlled`] has no `nominate`. This
    /// walks a whole controlled session to show that nothing routes round that.
    #[test]
    fn a_controlled_agent_never_sends_use_candidate() {
        let mut subject = agent(false, 100, &[address(BOB)]);
        let mut seen = 0usize;
        for _ in 0..6 {
            let outputs = subject.handle(Input::TimerFired(Timer::Ta));
            for message in requests(&outputs) {
                assert!(!message.use_candidate());
                assert!(matches!(
                    message.role(),
                    Some(RoleAttribute::Controlled { .. })
                ));
                seen += 1;
            }
            // Answer everything, so the session actually gets somewhere.
            for message in requests(&outputs) {
                let response =
                    stun::check_success(message.transaction(), &peer(), address(ALICE)).unwrap();
                deliver(&mut subject, address(BOB), response);
            }
        }
        assert!(seen > 0, "the controlled agent still sends checks");
    }

    /// §8.1.1's tolerance clause: a peer implemented against RFC 5245 may nominate more than
    /// once, and "the agents MUST produce the selected pairs and use the pairs with the highest
    /// priority". Tolerating a legacy peer is not the same as being one.
    ///
    /// Ta is fired only when the agent has armed one, because the interesting half of this is
    /// that the second nomination arrives *after* the checklist concluded and its triggered check
    /// therefore has to arm a tick of its own. A test that fires Ta by hand passes without that.
    #[test]
    fn a_peer_that_nominates_twice_selects_the_highest_priority_nominated_pair() {
        let low = address("198.51.100.3:6000");
        let high = address("198.51.100.2:6000");
        let mut subject = Agent::new(Config::default(), false, credentials("aaaa"), 100);
        let mut driver = Driver::default();
        driver.absorb(&subject.handle(Input::LocalCandidate(host(address(ALICE)))));
        driver.absorb(&subject.handle(Input::RemoteDescription {
            credentials: credentials("bbbb"),
            candidates: vec![
                Candidate::parse(&format!(
                    "1 1 UDP 1000 {} {} typ host",
                    low.ip(),
                    low.port()
                ))
                .unwrap(),
                Candidate::parse(&format!(
                    "2 1 UDP 2130706431 {} {} typ host",
                    high.ip(),
                    high.port()
                ))
                .unwrap(),
            ],
            lite: false,
        }));
        driver.absorb(&subject.handle(Input::GatheringDone));

        // The peer nominates the low-priority path first, then the high-priority one.
        for remote in [low, high] {
            let nominating = stun::connectivity_check(
                stun::new_transaction_id(),
                &peer(),
                Priority::new(1_862_270_975).unwrap(),
                RoleAttribute::Controlling {
                    tiebreaker: 999,
                    nominate: true,
                },
            )
            .unwrap();
            driver.absorb(&deliver(&mut subject, remote, nominating));

            let mut rounds = 0;
            while driver.armed && rounds < 10 {
                rounds += 1;
                let outputs = driver.tick(&mut subject);
                let destinations: Vec<(SocketAddr, TransactionId)> = outputs
                    .iter()
                    .filter_map(|output| match output {
                        Output::Send { to, bytes, .. } => Message::decode(bytes)
                            .ok()
                            .filter(|message| message.class() == Class::Request)
                            .map(|message| (*to, message.transaction())),
                        _ => None,
                    })
                    .collect();
                for (to, transaction) in destinations {
                    let response =
                        stun::check_success(transaction, &peer(), address(ALICE)).unwrap();
                    driver.absorb(&deliver(&mut subject, to, response));
                }
            }
        }

        assert_eq!(
            subject.selected(ComponentId::RTP),
            Some((LocalBase(0), high)),
            "§8.1.1: use the pair with the highest priority among the nominated ones"
        );
    }

    // ----------------------------------------------------------- §6.1.2.5 on the learning path

    /// §6.1.2.5's limit is a MUST and §19.5.1 is the attack it names. It binds at formation *and*
    /// on §7.3.1.4's insertion path, which is the one a peer drives: without it, each
    /// authenticated check from a fresh source address buys the sender a remote candidate, a
    /// pair, and eventually an 88-byte connectivity check sent to an address it named and need
    /// not be able to receive at.
    #[test]
    fn a_flood_of_checks_from_new_addresses_cannot_grow_the_set_past_the_limit() {
        let config = Config {
            pair_limit: 4,
            ..Config::default()
        };
        let mut subject = Agent::new(config, false, credentials("aaaa"), 100);
        let mut driver = Driver::default();
        driver.absorb(&subject.handle(Input::LocalCandidate(host(address(ALICE)))));
        driver.absorb(&subject.handle(Input::RemoteDescription {
            credentials: credentials("bbbb"),
            candidates: vec![host_line(address(BOB), "1")],
            lite: false,
        }));
        driver.absorb(&subject.handle(Input::GatheringDone));

        let mut answered = 0usize;
        for n in 0..200u32 {
            let source = address(&format!("198.51.100.{}:{}", n % 200 + 1, 40000 + n));
            let outputs = deliver(
                &mut subject,
                source,
                peer_check(RoleAttribute::Controlling {
                    tiebreaker: 999,
                    nominate: false,
                }),
            );
            driver.absorb(&outputs);
            answered += sent(&outputs).len();
        }

        assert_eq!(
            answered, 200,
            "§7.3 still answers every authenticated check — a 64-byte response to an 88-byte \
             request is not an amplifier"
        );
        assert!(
            subject.checklists().total_pairs() <= 4,
            "the checklist set grew to {} against a configured limit of 4",
            subject.checklists().total_pairs()
        );
        assert!(
            subject.remote_candidates().len() <= 5,
            "the remote candidate table grew to {}",
            subject.remote_candidates().len()
        );

        // And what the agent goes on to *send* is bounded by the limit, not by the flood.
        let mut destinations: Vec<SocketAddr> = Vec::new();
        let mut rounds = 0;
        while driver.armed && rounds < 200 {
            rounds += 1;
            for output in driver.tick(&mut subject) {
                if let Output::Send { to, bytes, .. } = output
                    && Message::decode(&bytes).is_ok_and(|m| m.class() == Class::Request)
                    && !destinations.contains(&to)
                {
                    destinations.push(to);
                }
            }
        }
        assert!(
            destinations.len() <= 4,
            "checks went to {} distinct addresses against a limit of 4",
            destinations.len()
        );
    }

    // ------------------------------------------------------------------- a second description

    fn remote_address_of(subject: &Agent, pair: PairId) -> Option<SocketAddr> {
        let remote = subject.checklists().pair(pair)?.remote;
        find_remote(subject.remote_candidates(), remote).map(|candidate| candidate.address)
    }

    fn three_remote_agent() -> (Agent, Driver, Vec<SocketAddr>) {
        let remotes: Vec<SocketAddr> = (1..=3)
            .map(|n| address(&format!("198.51.100.{n}:6000")))
            .collect();
        let mut subject = Agent::new(Config::default(), true, credentials("aaaa"), 100);
        let mut driver = Driver::default();
        driver.absorb(&subject.handle(Input::LocalCandidate(host(address(ALICE)))));
        driver.absorb(
            &subject.handle(Input::RemoteDescription {
                credentials: credentials("bbbb"),
                candidates: remotes
                    .iter()
                    .enumerate()
                    .map(|(index, remote)| host_line(*remote, &(index + 1).to_string()))
                    .collect(),
                lite: false,
            }),
        );
        driver.absorb(&subject.handle(Input::GatheringDone));
        (subject, driver, remotes)
    }

    /// RFC 8839 §4.2 lets a peer send more than one description for the same ICE session — a 183
    /// with SDP and then a 200 with SDP, or any re-INVITE — and the candidate list is the peer's
    /// to choose. Replacing the remote table under the live pairs leaves each of them naming a
    /// candidate it was never formed for, or nothing at all, and an agent whose pairs all dangle
    /// sends no checks, reports no failure and is simply silent.
    #[test]
    fn a_second_description_adds_candidates_without_re_pointing_the_live_pairs() {
        let (mut subject, mut driver, remotes) = three_remote_agent();
        let before: Vec<(PairId, SocketAddr)> = subject.checklists().checklists()[0]
            .pairs()
            .iter()
            .map(|pair| (pair.id, remote_address_of(&subject, pair.id).unwrap()))
            .collect();
        assert_eq!(before.len(), 3);

        let fresh = address("203.0.113.99:6000");
        driver.absorb(&subject.handle(Input::RemoteDescription {
            credentials: credentials("bbbb"),
            candidates: vec![host_line(fresh, "9")],
            lite: false,
        }));

        for (pair, was) in &before {
            assert_eq!(
                remote_address_of(&subject, *pair),
                Some(*was),
                "a re-offer must not re-point a pair that is already being checked"
            );
        }
        assert!(
            subject
                .remote_candidates()
                .iter()
                .any(|candidate| candidate.address == fresh),
            "the candidate the second description brought is added"
        );
        for remote in &remotes {
            assert!(
                subject
                    .remote_candidates()
                    .iter()
                    .any(|candidate| candidate.address == *remote),
                "a candidate the second description omitted is not dropped underneath its pair"
            );
        }

        // And the agent is still checking: it has a Ta armed and checks still leave.
        assert!(driver.armed);
        let destinations: Vec<SocketAddr> = driver
            .tick(&mut subject)
            .iter()
            .filter_map(|output| match output {
                Output::Send { to, bytes, .. } if Message::decode(bytes).is_ok() => Some(*to),
                _ => None,
            })
            .collect();
        assert_eq!(
            destinations.len(),
            1,
            "a re-offer must not silence the agent"
        );
    }

    /// RFC 8839 §4.4.1.1.1: **both** `ice-ufrag` and `ice-pwd` changing is an ICE restart, and
    /// everything is rebuilt for the new session.
    #[test]
    fn an_ice_restart_rebuilds_the_checklists_and_keeps_checking() {
        let (mut subject, mut driver, _) = three_remote_agent();
        let before: Vec<PairId> = subject.checklists().checklists()[0]
            .pairs()
            .iter()
            .map(|pair| pair.id)
            .collect();

        let fresh = address("203.0.113.99:6000");
        driver.absorb(&subject.handle(Input::RemoteDescription {
            credentials: credentials("cccc"),
            candidates: vec![host_line(fresh, "1")],
            lite: false,
        }));

        assert_eq!(subject.remote_candidates().len(), 1);
        assert_eq!(subject.checklists().total_pairs(), 1);
        for pair in before {
            assert!(
                subject.checklists().pair(pair).is_none(),
                "a restart is a new ICE session, so none of the old pairs survive it"
            );
        }
        assert!(driver.armed);
        let destinations: Vec<SocketAddr> = driver
            .tick(&mut subject)
            .iter()
            .filter_map(|output| match output {
                Output::Send { to, bytes, .. } if Message::decode(bytes).is_ok() => Some(*to),
                _ => None,
            })
            .collect();
        assert_eq!(destinations, vec![fresh]);
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
        let (mut alice, mut bob, mut left, mut right) = both_controlling(0x1234_5678_9abc_def0);
        assert_eq!(alice.role(), Role::Controlling);
        assert_eq!(bob.role(), Role::Controlling);

        exchange(&mut alice, &mut bob, &mut left, &mut right, 6);

        assert_ne!(
            alice.role(),
            bob.role(),
            "two controlling agents never converge: neither accepts the other's nomination"
        );
        assert!(alice.role().is_controlling() || bob.role().is_controlling());
    }
}
