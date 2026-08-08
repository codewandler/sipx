//! Checklists: pairing, ordering, pruning and pair state (RFC 8445 §6; [spec] §6).
//!
//! One checklist per data stream, and the ordered list of them is the "checklist set" §6.1.2.6
//! computes initial states over. sipx builds one checklist per [`Agent`](super::Agent) because a
//! media session is one data stream — but the set is a set, not a special case of one, because
//! §6.1.2.6's rule is *about* the set: a foundation already unfrozen in one checklist is not
//! unfrozen again in another, which is a sentence that has no meaning with a single checklist and
//! is the difference between checking a path once and checking it three times.
//!
//! [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md

use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};

use sipx_sdp::ice::{CandidateType, ComponentId};

use super::candidate::{
    LocalCandidate, LocalId, PairFoundation, RemoteCandidate, RemoteId, find_local, find_remote,
    pair_priority,
};

/// Which end decides (§6.1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Responsible for nominating the pairs that become the selected pairs.
    Controlling,
    /// Answers checks and follows the controlling agent's nomination.
    Controlled,
}

impl Role {
    /// §6.1.1's role determination, for an agent that is always full ([spec] §12).
    ///
    /// Both full: the initiating agent controls. Full against lite: the full agent controls,
    /// unconditionally — which is why the peer's `a=ice-lite` is an input here and not a detail
    /// for the driver.
    ///
    /// "The offerer controls" is the right answer and the wrong mechanism: two agents can both
    /// believe they offered — third-party call control, glare, a re-INVITE crossing — and two
    /// controlling agents never converge, because neither will accept the other's nomination.
    /// §7.3.1.1 is what repairs that, and it is why this function is not the last word on the
    /// role.
    ///
    /// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
    #[must_use]
    pub const fn determine(offerer: bool, remote_lite: bool) -> Self {
        if remote_lite || offerer {
            Self::Controlling
        } else {
            Self::Controlled
        }
    }

    /// The other role. A 487 switches to it (§7.2.5.1), as does the losing side of §7.3.1.1.
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Controlling => Self::Controlled,
            Self::Controlled => Self::Controlling,
        }
    }

    /// Whether this is the controlling role.
    #[must_use]
    pub const fn is_controlling(self) -> bool {
        matches!(self, Self::Controlling)
    }
}

/// A pair's state (§6.1.2.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PairState {
    /// No check sent, and none may be sent until the pair is unfrozen.
    Frozen,
    /// No check sent, but the pair is not Frozen.
    Waiting,
    /// A check has been sent and the transaction is in progress.
    InProgress,
    /// A check was sent and produced a successful result.
    Succeeded,
    /// A check was sent and failed, or timed out.
    Failed,
}

impl PairState {
    /// Whether the pair has finished — §7.2.5.4 asks whether every pair is in one of these two.
    #[must_use]
    pub const fn is_final(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed)
    }
}

/// A checklist's state (§6.1.2.1).
///
/// `Completed` and not `Succeeded`: §6.1.2.1 names the states and §7.2.5.4 calls the same state
/// Succeeded in passing. The name that appears in the state definitions is the one used here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChecklistState {
    /// Neither Completed nor Failed yet. Checklists start here.
    Running,
    /// There is a nominated pair for every component of the data stream.
    Completed,
    /// Every pair is Failed or Succeeded and some component has no valid pair.
    Failed,
}

/// A pair's identity, stable across sorting, pruning and removal.
///
/// Positions are not: §6.1.2.3 re-sorts every checklist on a role change, §8.1.2 removes pairs
/// once a component is nominated, and a triggered-check queue holding indices into a list that
/// does both would name a different pair after either.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PairId(pub u32);

/// Allocates [`PairId`]s that are unique for the lifetime of an agent.
#[derive(Debug, Default)]
pub struct PairIds(u32);

impl PairIds {
    /// The next identity.
    pub fn allocate(&mut self) -> PairId {
        let id = PairId(self.0);
        self.0 = self.0.saturating_add(1);
        id
    }
}

/// One entry in a checklist (§6.1.2.2, figure 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidatePair {
    /// Its identity.
    pub id: PairId,
    /// The local candidate, by index into the agent's table.
    pub local: LocalId,
    /// The remote candidate, by index into the agent's table.
    pub remote: RemoteId,
    /// The component both candidates are for.
    pub component: ComponentId,
    /// The combination of the two candidates' foundations.
    pub foundation: PairFoundation,
    /// §6.1.2.3's pair priority. Recomputed on every role change, because `G` and `D` swap.
    pub priority: u64,
    /// Its state.
    pub state: PairState,
    /// Whether a check on this pair carried `USE-CANDIDATE` and succeeded (§7.2.5.3.4).
    pub nominated: bool,
}

impl CandidatePair {
    /// Whether §6.1.2.5 may discard this pair.
    ///
    /// A pair with a check in flight or a check that succeeded is holding state outside the
    /// checklist — a transaction, or an entry in the valid list — and removing it silently would
    /// lose that rather than bound anything.
    #[must_use]
    pub const fn is_discardable(&self) -> bool {
        matches!(
            self.state,
            PairState::Frozen | PairState::Waiting | PairState::Failed
        ) && !self.nominated
    }
}

/// A pair in a valid list (§7.2.5.3.2).
///
/// Not a [`CandidatePair`], and deliberately so: §7.2.5.3.2 builds it from the *mapped address*
/// of the response and the address the request was sent to, so "it will be very common that the
/// valid pair will not be in any checklist" — its local candidate is the reflexive address a NAT
/// showed us, and every checklist pair had its reflexive locals replaced by their bases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidPair {
    /// The component it serves.
    pub component: ComponentId,
    /// The local candidate the check went out from — which is what the driver must send on.
    pub local: LocalId,
    /// The remote address the check was sent to.
    pub remote: SocketAddr,
    /// Its pair priority (§6.1.2.3).
    pub priority: u64,
    /// Whether it has been nominated (§7.2.5.3.4, §7.3.1.5).
    pub nominated: bool,
    /// The checklist pair whose check produced it.
    pub generated_by: PairId,
}

/// One data stream's checklist, its triggered-check queue and its valid list.
#[derive(Debug, Default)]
pub struct Checklist {
    pairs: Vec<CandidatePair>,
    state: Option<ChecklistState>,
    triggered: VecDeque<PairId>,
    valid: Vec<ValidPair>,
}

impl Checklist {
    /// A checklist over these pairs, before §6.1.2.6 has set any state.
    #[must_use]
    pub fn new(pairs: Vec<CandidatePair>) -> Self {
        Self {
            pairs,
            state: None,
            triggered: VecDeque::new(),
            valid: Vec::new(),
        }
    }

    /// The pairs, in checklist order.
    #[must_use]
    pub fn pairs(&self) -> &[CandidatePair] {
        &self.pairs
    }

    /// The pair with this identity.
    #[must_use]
    pub fn pair(&self, id: PairId) -> Option<&CandidatePair> {
        self.pairs.iter().find(|pair| pair.id == id)
    }

    /// The pair with this identity, mutably.
    pub fn pair_mut(&mut self, id: PairId) -> Option<&mut CandidatePair> {
        self.pairs.iter_mut().find(|pair| pair.id == id)
    }

    /// The checklist's state. `Running` until §6.1.2.6 has run.
    #[must_use]
    pub fn state(&self) -> ChecklistState {
        self.state.unwrap_or(ChecklistState::Running)
    }

    /// Set the checklist's state (§7.2.5.4, §8.1.2).
    pub fn set_state(&mut self, state: ChecklistState) {
        self.state = Some(state);
    }

    /// The valid list (§7.2.5.3.2).
    #[must_use]
    pub fn valid(&self) -> &[ValidPair] {
        &self.valid
    }

    /// Add a valid pair, or update the one already there for the same local and remote.
    ///
    /// Returns whether it was new, which is what arms [spec] §8's `Tn`: the stopping criterion
    /// counts from the *first* valid pair, and a retransmitted check that revalidates a pair
    /// already in the list must not restart it.
    ///
    /// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
    pub fn add_valid(&mut self, pair: ValidPair) -> bool {
        if let Some(existing) = self
            .valid
            .iter_mut()
            .find(|known| known.local == pair.local && known.remote == pair.remote)
        {
            existing.nominated |= pair.nominated;
            return false;
        }
        self.valid.push(pair);
        true
    }

    /// Mark the valid pair a check produced as nominated (§7.2.5.3.4).
    pub fn nominate_valid(&mut self, generated_by: PairId) {
        for valid in &mut self.valid {
            if valid.generated_by == generated_by {
                valid.nominated = true;
            }
        }
    }

    /// Drop a valid pair whose nominated check later failed (§7.2.5.3.4).
    pub fn remove_valid(&mut self, generated_by: PairId) {
        self.valid
            .retain(|valid| valid.generated_by != generated_by);
    }

    /// Enqueue a triggered check (§7.3.1.4). A pair already queued is not queued twice.
    pub fn trigger(&mut self, id: PairId) {
        if !self.triggered.contains(&id) {
            self.triggered.push_back(id);
        }
    }

    /// Take the next triggered check. §6.1.4.1's queue is FIFO, and §6.1.4.2 empties it before it
    /// looks at any `Waiting` pair — which is what makes ICE converge in the time it takes a
    /// peer's check to arrive rather than in checklist order.
    pub fn take_triggered(&mut self) -> Option<PairId> {
        self.triggered.pop_front()
    }

    /// Whether anything at all is queued for a triggered check.
    #[must_use]
    pub fn has_triggered(&self) -> bool {
        !self.triggered.is_empty()
    }

    /// Whether a triggered check is queued for this pair.
    #[must_use]
    pub fn is_triggered(&self, id: PairId) -> bool {
        self.triggered.contains(&id)
    }

    /// The pair joining these two candidates, if the checklist has one.
    #[must_use]
    pub fn find(&self, local: LocalId, remote: RemoteId) -> Option<PairId> {
        self.pairs
            .iter()
            .find(|pair| pair.local == local && pair.remote == remote)
            .map(|pair| pair.id)
    }

    /// Insert a pair §7.3.1.4 built from an inbound check, "based on its priority".
    pub fn insert(&mut self, pair: CandidatePair) {
        let position = self
            .pairs
            .iter()
            .position(|existing| existing.priority < pair.priority)
            .unwrap_or(self.pairs.len());
        self.pairs.insert(position, pair);
    }

    /// Sort into decreasing pair priority (§6.1.2.3).
    ///
    /// Stable, so that equal priorities keep the order they were formed in: §6.1.2.3 says ties
    /// are ordered arbitrarily, and a test that asserts on a checklist needs the same arbitrary
    /// answer twice.
    pub fn sort(&mut self) {
        self.pairs
            .sort_by_key(|pair| std::cmp::Reverse(pair.priority));
    }

    /// The components this checklist has pairs for.
    #[must_use]
    pub fn components(&self) -> Vec<ComponentId> {
        let mut components: Vec<ComponentId> =
            self.pairs.iter().map(|pair| pair.component).collect();
        components.sort_unstable();
        components.dedup();
        components
    }

    /// Remove every pair for a component but the one just nominated (§8.1.2).
    pub fn keep_only_nominated(&mut self, component: ComponentId, keep: PairId) {
        self.pairs
            .retain(|pair| pair.component != component || pair.id == keep);
        let live: Vec<PairId> = self.pairs.iter().map(|pair| pair.id).collect();
        self.triggered.retain(|id| live.contains(id));
    }
}

/// The ordered set of checklists, one per data stream (§6.1.2, §6.1.2.6).
#[derive(Debug, Default)]
pub struct ChecklistSet {
    checklists: Vec<Checklist>,
    next: usize,
}

impl ChecklistSet {
    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a checklist. The order is the "usage-defined checklist set order" §6.1.2.6 unfreezes
    /// against.
    pub fn push(&mut self, checklist: Checklist) {
        self.checklists.push(checklist);
    }

    /// The checklists, in order.
    #[must_use]
    pub fn checklists(&self) -> &[Checklist] {
        &self.checklists
    }

    /// The checklists, mutably.
    pub fn checklists_mut(&mut self) -> &mut [Checklist] {
        &mut self.checklists
    }

    /// Whether the set holds no checklists at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.checklists.is_empty()
    }

    /// The pair with this identity, wherever it is.
    #[must_use]
    pub fn pair(&self, id: PairId) -> Option<&CandidatePair> {
        self.checklists.iter().find_map(|list| list.pair(id))
    }

    /// The pair with this identity, mutably.
    pub fn pair_mut(&mut self, id: PairId) -> Option<&mut CandidatePair> {
        self.checklists
            .iter_mut()
            .find_map(|list| list.pair_mut(id))
    }

    /// Which checklist holds this pair.
    #[must_use]
    pub fn checklist_of(&self, id: PairId) -> Option<usize> {
        self.checklists
            .iter()
            .position(|list| list.pair(id).is_some())
    }

    /// `N` in §14.3's RTO: the total number of connectivity checks to be performed.
    #[must_use]
    pub fn total_pairs(&self) -> usize {
        self.checklists.iter().map(|list| list.pairs.len()).sum()
    }

    /// `Num-Waiting + Num-In-Progress` in §14.3's RTO, across the set.
    #[must_use]
    pub fn outstanding(&self) -> usize {
        self.checklists
            .iter()
            .flat_map(|list| list.pairs.iter())
            .filter(|pair| matches!(pair.state, PairState::Waiting | PairState::InProgress))
            .count()
    }

    /// §6.1.2.5: discard the lowest-priority pairs until the set holds at most `limit` of them.
    ///
    /// The limit is an attack control and not tidiness — it bounds how many packets a hostile
    /// candidate list can make sipx send — which is why §6.1.2.5 makes it a MUST and makes it
    /// configurable. The discarding is spread across checklists ("SHOULD be done evenly so that
    /// the number of candidate pairs in each checklist is reduced the same amount") by always
    /// taking from the longest checklist.
    ///
    /// **Only pairs that have not been checked, or have finished failing, are discardable.**
    /// §6.1.2.5 runs at checklist formation, when every pair is Frozen; this runs again every
    /// time §7.3.1.4 inserts a pair, because that is the path a peer can drive. Discarding an
    /// `In-Progress` pair would orphan its transaction, and discarding a `Succeeded` one would
    /// take a working path out of the valid list — so a set already full of live pairs stops
    /// shrinking rather than tearing itself down, and the limit then binds by refusing growth.
    pub fn limit(&mut self, limit: usize) {
        while self.total_pairs() > limit {
            let longest = self
                .checklists
                .iter()
                .enumerate()
                .filter(|(_, list)| list.pairs.iter().any(CandidatePair::is_discardable))
                .max_by_key(|(_, list)| list.pairs.len())
                .map(|(index, _)| index);
            let Some(index) = longest else { return };
            let Some(list) = self.checklists.get_mut(index) else {
                return;
            };
            let lowest = list
                .pairs
                .iter()
                .enumerate()
                .filter(|(_, pair)| pair.is_discardable())
                .min_by_key(|(_, pair)| pair.priority)
                .map(|(position, _)| position);
            match lowest {
                Some(position) => {
                    let dropped = list.pairs.remove(position);
                    list.triggered.retain(|id| *id != dropped.id);
                }
                None => return,
            }
        }
    }

    /// §6.1.2.6's initial states: everything Frozen, every checklist Running, and then exactly
    /// one pair per foundation moved to Waiting.
    ///
    /// The pair to unfreeze is "the first candidate pair (ordered by the lowest component ID and
    /// then the highest priority if component IDs are equal) in the first checklist … that has
    /// that foundation", and a foundation already unfrozen in an earlier checklist is not
    /// unfrozen again. RFC 8445's own Table 1 walks the case that distinguishes this from
    /// RFC 5245's rule, and `the_rfcs_three_checklist_five_foundation_example_unfreezes_five_pairs`
    /// asserts it cell by cell.
    pub fn compute_initial_states(&mut self) {
        for list in &mut self.checklists {
            for pair in &mut list.pairs {
                pair.state = PairState::Frozen;
            }
            list.set_state(ChecklistState::Running);
        }
        self.unfreeze_added();
    }

    /// §6.1.2, applied to a set that has grown: "if candidates are added to a checklist … the
    /// agent will re-perform these steps for the updated checklist".
    ///
    /// The same rule as §6.1.2.6 step 4, expressed over whatever is Frozen now: for each
    /// foundation that has no pair anywhere in the set outside the Frozen state, the first Frozen
    /// pair with it — by lowest component ID, then highest priority, in the first checklist that
    /// has it — moves to Waiting. Run over a set where everything is Frozen this *is* step 4; run
    /// over a live set it unfreezes exactly the foundations the new pairs brought.
    pub fn unfreeze_added(&mut self) {
        let mut unfrozen: Vec<PairFoundation> = self
            .checklists
            .iter()
            .flat_map(|list| list.pairs.iter())
            .filter(|pair| pair.state != PairState::Frozen)
            .map(|pair| pair.foundation.clone())
            .collect();

        for list in &mut self.checklists {
            let mut order: Vec<usize> = (0..list.pairs.len()).collect();
            order.sort_by_key(|position| {
                list.pairs
                    .get(*position)
                    .map(|pair| (pair.component, std::cmp::Reverse(pair.priority)))
            });
            for position in order {
                let Some(pair) = list.pairs.get_mut(position) else {
                    continue;
                };
                if pair.state != PairState::Frozen || unfrozen.contains(&pair.foundation) {
                    continue;
                }
                pair.state = PairState::Waiting;
                unfrozen.push(pair.foundation.clone());
            }
        }
    }

    /// §7.2.5.3.3: every Frozen pair in every checklist that shares this foundation moves to
    /// Waiting.
    pub fn unfreeze_foundation(&mut self, foundation: &PairFoundation) {
        for list in &mut self.checklists {
            for pair in &mut list.pairs {
                if pair.state == PairState::Frozen && pair.foundation == *foundation {
                    pair.state = PairState::Waiting;
                }
            }
        }
    }

    /// §6.1.4.2 step 2: with nothing Waiting in this checklist and something Frozen in it,
    /// unfreeze the Frozen pairs whose foundation has no pair Waiting or In-Progress anywhere in
    /// the set.
    ///
    /// This is the second unfreeze trigger, and the one that is easy to miss. Without it, a
    /// foundation whose one unfrozen pair failed leaves its remaining pairs Frozen for the rest
    /// of the session — §6.1.2.6 unfreezes each foundation exactly once, and §7.2.5.3.3 only ever
    /// unfreezes on *success* — so ICE reports a failure for a path it never finished checking.
    pub fn unfreeze_idle(&mut self, index: usize) {
        let Some(list) = self.checklists.get(index) else {
            return;
        };
        if list
            .pairs
            .iter()
            .any(|pair| pair.state == PairState::Waiting)
        {
            return;
        }
        let frozen: Vec<PairFoundation> = list
            .pairs
            .iter()
            .filter(|pair| pair.state == PairState::Frozen)
            .map(|pair| pair.foundation.clone())
            .collect();
        let busy: Vec<PairFoundation> = self
            .checklists
            .iter()
            .flat_map(|list| list.pairs.iter())
            .filter(|pair| matches!(pair.state, PairState::Waiting | PairState::InProgress))
            .map(|pair| pair.foundation.clone())
            .collect();

        let Some(list) = self.checklists.get_mut(index) else {
            return;
        };
        let mut thawed: Vec<PairFoundation> = Vec::new();
        for foundation in frozen {
            if busy.contains(&foundation) || thawed.contains(&foundation) {
                continue;
            }
            if let Some(pair) = list
                .pairs
                .iter_mut()
                .find(|pair| pair.state == PairState::Frozen && pair.foundation == foundation)
            {
                pair.state = PairState::Waiting;
                thawed.push(foundation);
            }
        }
    }

    /// The next checklist Ta may act on, round-robin (§6.1.4.2).
    ///
    /// "Whenever Ta fires the next checklist in the Running state in the checklist set is picked
    /// … After the last checklist in the Running state has been processed, the first checklist is
    /// picked again." A Completed checklist is included when it still has a triggered check
    /// queued, because §8.1.2 requires an agent to keep answering for a concluded stream and
    /// §8.1.1's tolerance clause depends on it.
    pub fn next_active(&mut self) -> Option<usize> {
        let count = self.checklists.len();
        for offset in 0..count {
            let index = (self.next.wrapping_add(offset)) % count.max(1);
            if self
                .checklists
                .get(index)
                .is_some_and(|list| list.state() == ChecklistState::Running || list.has_triggered())
            {
                self.next = index.wrapping_add(1) % count.max(1);
                return Some(index);
            }
        }
        None
    }

    /// Recompute every pair priority and re-sort every checklist (§6.1.2.3).
    ///
    /// A role change swaps which side is `G` and which is `D`, so this runs on every role change —
    /// forgetting it is one of the two ways role conflict is mishandled, and the other is not
    /// detecting the conflict at all.
    pub fn recompute_priorities(
        &mut self,
        role: Role,
        locals: &[LocalCandidate],
        remotes: &[RemoteCandidate],
    ) {
        for list in &mut self.checklists {
            for pair in &mut list.pairs {
                let (Some(local), Some(remote)) = (
                    find_local(locals, pair.local),
                    find_remote(remotes, pair.remote),
                ) else {
                    continue;
                };
                pair.priority = ordered_pair_priority(role, local.priority, remote.priority);
            }
            list.sort();
        }
        for list in &mut self.checklists {
            for valid in &mut list.valid {
                let Some(local) = find_local(locals, valid.local) else {
                    continue;
                };
                let remote = remotes
                    .iter()
                    .find(|candidate| candidate.address == valid.remote)
                    .map(|candidate| candidate.priority);
                if let Some(remote) = remote {
                    valid.priority = ordered_pair_priority(role, local.priority, remote);
                }
            }
        }
    }
}

/// §6.1.2.3's pair priority with the operands put in the roles the formula names.
///
/// `G` is "the priority for the candidate provided by the controlling agent" — so which of the
/// two candidate priorities is `G` is a fact about our role, not about the pair.
#[must_use]
pub fn ordered_pair_priority(
    role: Role,
    local: sipx_sdp::ice::Priority,
    remote: sipx_sdp::ice::Priority,
) -> u64 {
    if role.is_controlling() {
        pair_priority(local, remote)
    } else {
        pair_priority(remote, local)
    }
}

/// Whether an address is an IPv6 link-local unicast address (`fe80::/10`).
///
/// §6.1.2.2 makes pairing one with anything but another link-local address a MUST NOT: with IPv6
/// a host commonly has several addresses per interface, and a link-local paired with a global one
/// is a check that cannot work and a packet that was sent anyway.
#[must_use]
pub fn is_link_local(address: IpAddr) -> bool {
    match address {
        IpAddr::V6(v6) => v6
            .segments()
            .first()
            .is_some_and(|first| *first & 0xffc0 == 0xfe80),
        IpAddr::V4(_) => false,
    }
}

/// §6.1.2.2: pair each local candidate with each remote candidate of the same component and the
/// same address family, then §6.1.2.4's pruning.
///
/// The component reduction is §6.1.2.2's: "the number of components for that data stream is
/// effectively reduced … to the minimum across both agents of the maximum component ID provided
/// by each agent". If sipx offers RTP alone because [`MediaPort`](crate::session) did not get the
/// control port, the peer's RTCP candidates go unpaired, which is exactly the case that sentence
/// describes.
pub fn form_pairs(
    ids: &mut PairIds,
    role: Role,
    locals: &[LocalCandidate],
    remotes: &[RemoteCandidate],
) -> Vec<CandidatePair> {
    let max_local = locals
        .iter()
        .map(|candidate| candidate.gathered.component)
        .max();
    let max_remote = remotes.iter().map(|candidate| candidate.component).max();
    let (Some(max_local), Some(max_remote)) = (max_local, max_remote) else {
        return Vec::new();
    };
    let ceiling = max_local.min(max_remote);

    let mut pairs = Vec::new();
    for local in locals {
        if local.gathered.component > ceiling {
            continue;
        }
        for remote in remotes {
            if local.gathered.component != remote.component {
                continue;
            }
            let local_ip = local.gathered.address.ip();
            let remote_ip = remote.address.ip();
            if local_ip.is_ipv4() != remote_ip.is_ipv4() {
                continue;
            }
            if is_link_local(local_ip) != is_link_local(remote_ip) {
                continue;
            }
            pairs.push(CandidatePair {
                id: ids.allocate(),
                local: local.id,
                remote: remote.id,
                component: local.gathered.component,
                foundation: PairFoundation {
                    local: local.foundation,
                    remote: remote.foundation.clone(),
                },
                priority: ordered_pair_priority(role, local.priority, remote.priority),
                state: PairState::Frozen,
                nominated: false,
            });
        }
    }
    pairs.sort_by_key(|pair| std::cmp::Reverse(pair.priority));
    prune(&mut pairs, locals, remotes);
    pairs
}

/// §6.1.2.4: replace a reflexive local candidate with its base, then drop redundant pairs.
///
/// Both halves matter and only one of them is obvious. A check is sent *from a base* — there is
/// no socket at a reflexive address — so a pair whose local candidate is reflexive names an
/// address nothing can send from. Replacing it then makes pairs collide, and the second half
/// removes the collisions: "two candidate pairs are redundant if their local candidates have the
/// same base and their remote candidates are identical", keeping the higher-priority one, which
/// is the first in an already-sorted list.
fn prune(pairs: &mut Vec<CandidatePair>, locals: &[LocalCandidate], remotes: &[RemoteCandidate]) {
    for pair in pairs.iter_mut() {
        let Some(local) = find_local(locals, pair.local) else {
            continue;
        };
        if !matches!(
            local.gathered.kind,
            CandidateType::ServerReflexive | CandidateType::PeerReflexive
        ) {
            continue;
        }
        let base = local.gathered.base_address;
        let component = local.gathered.component;
        if let Some(host) = locals.iter().find(|candidate| {
            candidate.gathered.kind == CandidateType::Host
                && candidate.gathered.address == base
                && candidate.gathered.component == component
        }) {
            pair.local = host.id;
        }
    }

    let mut kept: Vec<(SocketAddr, SocketAddr)> = Vec::new();
    pairs.retain(|pair| {
        let (Some(local), Some(remote)) = (
            find_local(locals, pair.local),
            find_remote(remotes, pair.remote),
        ) else {
            return false;
        };
        let key = (local.gathered.base_address, remote.address);
        if kept.contains(&key) {
            return false;
        }
        kept.push(key);
        true
    });
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use sipx_sdp::ice::{Foundation, Priority};

    use super::*;
    use crate::ice::candidate::{
        Gathered, LocalBase, LocalFoundation, RemoteFoundation, SINGLE_ADDRESS_PREFERENCE,
        assign_local_preferences,
    };
    use crate::ice::candidate::{find_local, find_remote};

    fn component(id: u16) -> ComponentId {
        ComponentId::new(id).unwrap()
    }

    fn host(id: usize, ip: &str, port: u16, component_id: u16) -> LocalCandidate {
        let address = SocketAddr::new(ip.parse().unwrap(), port);
        LocalCandidate {
            id: LocalId(id),
            gathered: Gathered {
                base: LocalBase(0),
                base_address: address,
                address,
                kind: CandidateType::Host,
                component: component(component_id),
                server: None,
            },
            foundation: LocalFoundation(1),
            local_preference: SINGLE_ADDRESS_PREFERENCE,
            priority: crate::ice::candidate::priority(
                crate::ice::candidate::HOST_PREFERENCE,
                SINGLE_ADDRESS_PREFERENCE,
                component(component_id),
            ),
        }
    }

    fn remote(ip: &str, port: u16, component_id: u16, foundation: u32) -> RemoteCandidate {
        RemoteCandidate {
            // Distinct per fixture: the address decides, as it does for a real candidate.
            id: RemoteId(ip.bytes().map(usize::from).sum::<usize>() * 100_000 + usize::from(port)),
            address: SocketAddr::new(ip.parse().unwrap(), port),
            kind: CandidateType::Host,
            component: component(component_id),
            foundation: RemoteFoundation::Signalled(
                Foundation::new(&foundation.to_string()).unwrap(),
            ),
            // Server-reflexive, so that a pair's `G` and `D` differ and a role swap is visible.
            priority: crate::ice::candidate::priority(
                crate::ice::candidate::SERVER_REFLEXIVE_PREFERENCE,
                SINGLE_ADDRESS_PREFERENCE,
                component(component_id),
            ),
        }
    }

    fn pair(id: u32, component_id: u16, foundation: u32, priority: u64) -> CandidatePair {
        CandidatePair {
            id: PairId(id),
            local: LocalId(0),
            remote: RemoteId(0),
            component: component(component_id),
            foundation: PairFoundation {
                local: LocalFoundation(foundation),
                remote: RemoteFoundation::Learned(foundation),
            },
            priority,
            state: PairState::Frozen,
            nominated: false,
        }
    }

    /// RFC 8445 §6.1.2.6's Table 1, cell by cell.
    ///
    /// Three checklists over five foundations: `m1` has f1, f2, f3; `m2` has f1, f2, f3, f4; `m3`
    /// has f1 and f5. Exactly five pairs end up Waiting — every pair in `m1`, f4 in `m2`, f5 in
    /// `m3` — and the rest stay Frozen because their foundation was already unfrozen in an
    /// earlier checklist. This is the case the RFC's own NOTE calls out as different from
    /// RFC 5245, where only the first checklist was ever unfrozen.
    #[test]
    fn the_rfcs_three_checklist_five_foundation_example_unfreezes_five_pairs() {
        let mut set = ChecklistSet::new();
        set.push(Checklist::new(vec![
            pair(1, 1, 1, 300),
            pair(2, 1, 2, 200),
            pair(3, 1, 3, 100),
        ]));
        set.push(Checklist::new(vec![
            pair(4, 1, 1, 300),
            pair(5, 1, 2, 200),
            pair(6, 1, 3, 100),
            pair(7, 1, 4, 50),
        ]));
        set.push(Checklist::new(vec![pair(8, 1, 1, 300), pair(9, 1, 5, 10)]));

        set.compute_initial_states();

        let state = |id: u32| set.pair(PairId(id)).unwrap().state;
        // m1: every foundation is new here, so every pair is unfrozen.
        assert_eq!(state(1), PairState::Waiting);
        assert_eq!(state(2), PairState::Waiting);
        assert_eq!(state(3), PairState::Waiting);
        // m2: f1, f2 and f3 were already unfrozen in m1; only f4 is new.
        assert_eq!(state(4), PairState::Frozen);
        assert_eq!(state(5), PairState::Frozen);
        assert_eq!(state(6), PairState::Frozen);
        assert_eq!(state(7), PairState::Waiting);
        // m3: f1 was unfrozen in m1; only f5 is new.
        assert_eq!(state(8), PairState::Frozen);
        assert_eq!(state(9), PairState::Waiting);

        assert!(
            set.checklists()
                .iter()
                .all(|list| list.state() == ChecklistState::Running)
        );
    }

    /// "ordered by the lowest component ID and then the highest priority if component IDs are
    /// equal": with both components sharing a foundation, it is the RTP pair that thaws.
    #[test]
    fn the_unfrozen_pair_for_a_foundation_is_the_lowest_component_then_the_highest_priority() {
        let mut set = ChecklistSet::new();
        set.push(Checklist::new(vec![
            pair(1, 2, 1, 900),
            pair(2, 1, 1, 100),
            pair(3, 1, 1, 500),
        ]));
        set.compute_initial_states();
        // Component 1 beats the higher-priority component 2 pair; within component 1, 500 wins.
        assert_eq!(set.pair(PairId(3)).unwrap().state, PairState::Waiting);
        assert_eq!(set.pair(PairId(1)).unwrap().state, PairState::Frozen);
        assert_eq!(set.pair(PairId(2)).unwrap().state, PairState::Frozen);
    }

    /// §6.1.4.2 step 2. The foundation's one unfrozen pair failed; without this the rest of the
    /// foundation stays Frozen for the session and ICE fails a path it never checked.
    #[test]
    fn a_foundation_whose_only_unfrozen_pair_failed_is_thawed_again() {
        let mut set = ChecklistSet::new();
        set.push(Checklist::new(vec![pair(1, 1, 1, 300), pair(2, 2, 1, 200)]));
        set.compute_initial_states();
        assert_eq!(set.pair(PairId(1)).unwrap().state, PairState::Waiting);
        assert_eq!(set.pair(PairId(2)).unwrap().state, PairState::Frozen);

        set.pair_mut(PairId(1)).unwrap().state = PairState::Failed;
        set.unfreeze_idle(0);
        assert_eq!(set.pair(PairId(2)).unwrap().state, PairState::Waiting);
    }

    /// …and it must not fire while the foundation still has a check in flight, or the pacing of
    /// §6.1.4.2 becomes two checks per Ta tick for the same foundation.
    #[test]
    fn nothing_is_thawed_while_the_foundation_still_has_a_check_outstanding() {
        let mut set = ChecklistSet::new();
        set.push(Checklist::new(vec![pair(1, 1, 1, 300), pair(2, 2, 1, 200)]));
        set.compute_initial_states();
        set.pair_mut(PairId(1)).unwrap().state = PairState::InProgress;
        set.unfreeze_idle(0);
        assert_eq!(set.pair(PairId(2)).unwrap().state, PairState::Frozen);
    }

    #[test]
    fn pairs_are_ordered_by_decreasing_priority_and_ties_keep_their_order() {
        let mut list = Checklist::new(vec![
            pair(1, 1, 1, 100),
            pair(2, 1, 2, 900),
            pair(3, 1, 3, 100),
        ]);
        list.sort();
        let order: Vec<u32> = list.pairs().iter().map(|pair| pair.id.0).collect();
        assert_eq!(order, vec![2, 1, 3]);
        list.sort();
        let again: Vec<u32> = list.pairs().iter().map(|pair| pair.id.0).collect();
        assert_eq!(order, again);
    }

    #[test]
    fn the_hundred_pair_limit_is_enforced_and_configurable() {
        let mut set = ChecklistSet::new();
        set.push(Checklist::new(
            (0..150u32).map(|n| pair(n, 1, 1, u64::from(n))).collect(),
        ));
        set.limit(100);
        assert_eq!(set.total_pairs(), 100);
        // The lowest-priority pairs went.
        assert!(set.pair(PairId(0)).is_none());
        assert!(set.pair(PairId(149)).is_some());

        set.limit(10);
        assert_eq!(set.total_pairs(), 10);
    }

    /// §6.1.2.5 wants the discarding spread across checklists, so a long checklist loses pairs
    /// before a short one does.
    #[test]
    fn the_limit_takes_from_the_longest_checklist_first() {
        let mut set = ChecklistSet::new();
        set.push(Checklist::new(
            (0..8u32).map(|n| pair(n, 1, 1, u64::from(n))).collect(),
        ));
        set.push(Checklist::new(
            (10..12u32).map(|n| pair(n, 1, 1, u64::from(n))).collect(),
        ));
        set.limit(6);
        assert_eq!(set.checklists()[0].pairs().len(), 4);
        assert_eq!(set.checklists()[1].pairs().len(), 2);
    }

    #[test]
    fn candidates_pair_only_within_a_component_and_an_address_family() {
        let locals = vec![host(1, "192.0.2.1", 5000, 1), host(2, "192.0.2.1", 5001, 2)];
        let remotes = vec![
            remote("198.51.100.1", 6000, 1, 1),
            remote("198.51.100.1", 6001, 2, 1),
            remote("2001:db8::1", 6002, 1, 2),
        ];
        let mut ids = PairIds::default();
        let pairs = form_pairs(&mut ids, Role::Controlling, &locals, &remotes);
        assert_eq!(pairs.len(), 2);
        assert!(pairs.iter().all(|pair| {
            find_local(&locals, pair.local).unwrap().gathered.component
                == find_remote(&remotes, pair.remote).unwrap().component
        }));
    }

    /// §6.1.2.2's MUST NOT: a link-local address pairs only with another link-local one.
    #[test]
    fn an_ipv6_link_local_candidate_pairs_only_with_link_local_addresses() {
        assert!(is_link_local("fe80::1".parse().unwrap()));
        assert!(!is_link_local("2001:db8::1".parse().unwrap()));
        assert!(!is_link_local("192.0.2.1".parse().unwrap()));

        let locals = vec![host(1, "fe80::1", 5000, 1), host(2, "2001:db8::1", 5000, 1)];
        let remotes = vec![
            remote("fe80::2", 6000, 1, 1),
            remote("2001:db8::2", 6000, 1, 2),
        ];
        let mut ids = PairIds::default();
        let pairs = form_pairs(&mut ids, Role::Controlling, &locals, &remotes);
        assert_eq!(pairs.len(), 2);
        for pair in &pairs {
            assert_eq!(
                is_link_local(
                    find_local(&locals, pair.local)
                        .unwrap()
                        .gathered
                        .address
                        .ip()
                ),
                is_link_local(find_remote(&remotes, pair.remote).unwrap().address.ip())
            );
        }
    }

    /// §6.1.2.2: the number of components is the minimum of the two agents' maxima, so a peer
    /// that offers RTCP to an agent that has no control port gets its RTCP candidates unpaired.
    #[test]
    fn a_peer_offering_rtcp_to_an_agent_without_one_gets_no_rtcp_pairs() {
        let locals = vec![host(1, "192.0.2.1", 5000, 1)];
        let remotes = vec![
            remote("198.51.100.1", 6000, 1, 1),
            remote("198.51.100.1", 6001, 2, 1),
        ];
        let mut ids = PairIds::default();
        let pairs = form_pairs(&mut ids, Role::Controlling, &locals, &remotes);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].component, component(1));
    }

    /// §6.1.2.4, both halves: the reflexive local becomes its base, and the pair that collides
    /// with the host pair as a result is dropped rather than checked twice.
    #[test]
    fn a_reflexive_local_becomes_its_base_and_the_redundant_pair_goes() {
        let base = SocketAddr::new("192.0.2.1".parse().unwrap(), 5000);
        let mut locals = vec![
            host(1, "192.0.2.1", 5000, 1),
            LocalCandidate {
                id: LocalId(1),
                gathered: Gathered {
                    base: LocalBase(0),
                    base_address: base,
                    address: SocketAddr::new("198.51.100.9".parse().unwrap(), 7000),
                    kind: CandidateType::ServerReflexive,
                    component: component(1),
                    server: Some("198.51.100.1".parse().unwrap()),
                },
                foundation: LocalFoundation(2),
                local_preference: SINGLE_ADDRESS_PREFERENCE,
                priority: Priority::new(1).unwrap(),
            },
        ];
        assign_local_preferences(&mut locals);
        let remotes = vec![remote("198.51.100.2", 6000, 1, 1)];

        let mut ids = PairIds::default();
        let pairs = form_pairs(&mut ids, Role::Controlling, &locals, &remotes);
        assert_eq!(pairs.len(), 1);
        // Whichever pair survived, its local candidate is one a socket exists at.
        assert_eq!(
            find_local(&locals, pairs[0].local).unwrap().gathered.kind,
            CandidateType::Host
        );
    }

    /// §6.1.2.3: `G` is the controlling agent's candidate, so the same pair seen from the two
    /// ends must produce the same number — which it only does if the role decides the operands.
    #[test]
    fn both_ends_compute_the_same_pair_priority_for_the_same_pair() {
        let ours = Priority::new(2_130_706_431).unwrap();
        let theirs = Priority::new(1_694_498_815).unwrap();
        assert_eq!(
            ordered_pair_priority(Role::Controlling, ours, theirs),
            ordered_pair_priority(Role::Controlled, theirs, ours)
        );
    }

    #[test]
    fn a_role_change_recomputes_every_pair_priority_and_re_sorts() {
        let locals = vec![host(1, "192.0.2.1", 5000, 1)];
        let remotes = vec![remote("198.51.100.1", 6000, 1, 1)];
        let mut ids = PairIds::default();
        let mut set = ChecklistSet::new();
        set.push(Checklist::new(form_pairs(
            &mut ids,
            Role::Controlling,
            &locals,
            &remotes,
        )));
        let before = set.checklists()[0].pairs()[0].priority;

        set.recompute_priorities(Role::Controlled, &locals, &remotes);
        let after = set.checklists()[0].pairs()[0].priority;
        // The two candidates have different priorities here, so swapping G and D must move it.
        assert_ne!(before, after);
        assert_eq!(
            after,
            ordered_pair_priority(Role::Controlled, locals[0].priority, remotes[0].priority)
        );
        // And the pair still names the candidates it was formed for.
        let pair = &set.checklists()[0].pairs()[0];
        assert!(find_local(&locals, pair.local).is_some());
        assert!(find_remote(&remotes, pair.remote).is_some());
    }

    #[test]
    fn the_triggered_queue_is_fifo_and_holds_a_pair_once() {
        let mut list = Checklist::new(vec![pair(1, 1, 1, 100), pair(2, 1, 2, 200)]);
        list.trigger(PairId(2));
        list.trigger(PairId(1));
        list.trigger(PairId(2));
        assert_eq!(list.take_triggered(), Some(PairId(2)));
        assert_eq!(list.take_triggered(), Some(PairId(1)));
        assert_eq!(list.take_triggered(), None);
    }

    #[test]
    fn determining_the_role_follows_section_6_1_1() {
        assert_eq!(Role::determine(true, false), Role::Controlling);
        assert_eq!(Role::determine(false, false), Role::Controlled);
        // Full against lite: controlling, whoever offered.
        assert_eq!(Role::determine(false, true), Role::Controlling);
        assert_eq!(Role::determine(true, true), Role::Controlling);
        assert_eq!(Role::Controlling.opposite(), Role::Controlled);
        assert_eq!(Role::Controlled.opposite(), Role::Controlling);
    }
}
