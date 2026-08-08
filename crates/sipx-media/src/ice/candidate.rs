//! Candidates as the agent holds them: priority, foundation and base
//! (RFC 8445 §5.1.1.3, §5.1.2.1, §7.1.1; [spec] §4, §5).
//!
//! [`sipx_sdp::ice::Candidate`] is the `a=candidate` line. It is what crosses the wire and it
//! knows nothing about which socket a check would leave from, because `sipx-sdp` owns no sockets.
//! The two types here add exactly that: a [`LocalCandidate`] carries the [`LocalBase`] the driver
//! bound, and a [`RemoteCandidate`] carries a foundation that a peer-reflexive candidate can also
//! have — §7.3.1.3 gives one "an arbitrary value, different from the foundations of all other
//! remote candidates", which is not a value any `a=candidate` line ever supplied.
//!
//! [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md

use std::net::{IpAddr, SocketAddr};

use sipx_sdp::ice::{Candidate, CandidateType, ComponentId, Foundation, Priority, Transport};

/// Type preference for a host candidate (§5.1.2.2's recommended value; [spec] §4).
///
/// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
pub const HOST_PREFERENCE: u8 = 126;
/// Type preference for a peer-reflexive candidate. §5.1.2.1 makes it a MUST that this is higher
/// than the server-reflexive one, and it is the preference every `PRIORITY` attribute uses
/// whatever the candidate actually is (§7.1.1, [`check_priority`]).
///
/// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
pub const PEER_REFLEXIVE_PREFERENCE: u8 = 110;
/// Type preference for a server-reflexive candidate (§5.1.2.2's recommended value).
pub const SERVER_REFLEXIVE_PREFERENCE: u8 = 100;
/// Type preference for a relayed candidate: last resort, because it costs a relay's bandwidth.
pub const RELAYED_PREFERENCE: u8 = 0;

/// The largest type preference §5.1.2.1 admits: "an integer from 0 … to 126 … inclusive".
pub const MAX_TYPE_PREFERENCE: u8 = 126;

/// The local preference for a candidate that is the only one of its type for its component.
/// §5.1.2.1: "When there is only a single IP address, this value SHOULD be set to 65535."
pub const SINGLE_ADDRESS_PREFERENCE: u16 = 65535;

/// Which socket the driver bound, named so the agent never has to hold one.
///
/// [spec] §2: "`LocalBase` is an index into the sockets the driver bound, not a socket. The agent
/// never learns what a socket is; it says 'the one you called base 0' and the driver knows which."
///
/// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LocalBase(pub u16);

/// A local candidate's identity.
///
/// A handle and not a copy of the candidate: §7.2.5.3.1 adds peer-reflexive candidates while
/// pairs are live, and a pair that had copied its candidate would still be holding the priority
/// it had before the role switched.
///
/// An allocated identity and not a position, for the same reason [`PairId`](crate::ice::checklist::PairId)
/// is: the tables these name are not append-only. A second offer replaces the remote candidates,
/// §6.1.2.5's limit discards pairs, and §7.3.1.3 learns candidates that may later be forgotten —
/// after any of which a stored position names a candidate the pair was never formed for, which is
/// a check sent to an address the peer never offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LocalId(pub usize);

/// A remote candidate's identity, allocated and stable — see [`LocalId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RemoteId(pub usize);

/// Allocates the identities of [`LocalCandidate`]s and [`RemoteCandidate`]s.
///
/// One counter each, never reused, so a stale handle resolves to nothing rather than to whatever
/// took the position.
#[derive(Debug, Default)]
pub struct CandidateIds {
    local: usize,
    remote: usize,
}

impl CandidateIds {
    /// The next local identity.
    pub fn local(&mut self) -> LocalId {
        let id = LocalId(self.local);
        self.local = self.local.saturating_add(1);
        id
    }

    /// The next remote identity.
    pub fn remote(&mut self) -> RemoteId {
        let id = RemoteId(self.remote);
        self.remote = self.remote.saturating_add(1);
        id
    }
}

/// The local candidate with this identity, if it is still known.
#[must_use]
pub fn find_local(candidates: &[LocalCandidate], id: LocalId) -> Option<&LocalCandidate> {
    candidates.iter().find(|candidate| candidate.id == id)
}

/// The remote candidate with this identity, if it is still known.
#[must_use]
pub fn find_remote(candidates: &[RemoteCandidate], id: RemoteId) -> Option<&RemoteCandidate> {
    candidates.iter().find(|candidate| candidate.id == id)
}

/// A local candidate's foundation (§5.1.1.3).
///
/// A decimal counter over the distinct tuples §5.1.1.3 defines, allocated in the order candidates
/// are gathered, because the value itself is arbitrary — RFC 8839 §5.1 wants `1*32ice-char` and
/// RFC 8445 gives the value meaning only by equality. A hash would satisfy the same grammar and
/// be longer on the wire for no gain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LocalFoundation(pub u32);

/// A remote candidate's foundation.
///
/// Two variants because a remote candidate has two provenances, and §7.3.1.3's is not expressible
/// as an `a=candidate` foundation: a peer-reflexive remote candidate is learned from a check, not
/// signalled, and its foundation is required to be "an arbitrary value, different from the
/// foundations of all other remote candidates". Making that a separate variant is what stops a
/// learned foundation from ever comparing equal to a signalled one by accident.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RemoteFoundation {
    /// The foundation the peer put on its `a=candidate` line (RFC 8839 §5.1).
    Signalled(Foundation),
    /// A counter allocated for a peer-reflexive remote candidate (§7.3.1.3).
    Learned(u32),
}

/// A candidate pair's foundation: §6.1.2.6's "combination of the foundations of the local and
/// remote candidates in the pair".
///
/// It exists only to be compared. §6.1.2.6 unfreezes exactly one pair per foundation and
/// §7.2.5.3.3 unfreezes every pair sharing the foundation of one that just succeeded, so a wrong
/// answer here makes ICE either check far too much or check nothing at all.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PairFoundation {
    /// The local candidate's foundation.
    pub local: LocalFoundation,
    /// The remote candidate's.
    pub remote: RemoteFoundation,
}

/// A candidate the driver gathered, before the agent has priced it.
///
/// The agent assigns the foundation and the priority rather than taking them, because both are
/// properties of the *set* of candidates: §5.1.1.3's foundation is a counter over distinct
/// tuples, and §5.1.2.1's local preference "MUST be unique for each" candidate of a type, which
/// is not a fact any single candidate knows about itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gathered {
    /// The socket this candidate was gathered on, and which a check would leave from.
    pub base: LocalBase,
    /// That socket's own address — the candidate's base in §5.1.1.1's sense.
    pub base_address: SocketAddr,
    /// The candidate's transport address. Equal to `base_address` for a host candidate; the
    /// address a STUN or TURN server reported otherwise.
    pub address: SocketAddr,
    /// How it was obtained.
    pub kind: CandidateType,
    /// Which component of the stream it is for.
    pub component: ComponentId,
    /// The IP address of the STUN or TURN server it was obtained from, for reflexive and relayed
    /// candidates. Part of the foundation (§5.1.1.3) and `None` for a host candidate.
    pub server: Option<IpAddr>,
}

/// A local candidate: what the driver gathered, plus what §5.1.1.3 and §5.1.2.1 make of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalCandidate {
    /// Its identity, stable for the life of the agent.
    pub id: LocalId,
    /// The gathered address and its base.
    pub gathered: Gathered,
    /// Its foundation (§5.1.1.3).
    pub foundation: LocalFoundation,
    /// Its local preference (§5.1.2.1), unique among candidates of the same type and component.
    pub local_preference: u16,
    /// Its priority (§5.1.2.1).
    pub priority: Priority,
}

impl LocalCandidate {
    /// The `PRIORITY` a connectivity check from this candidate carries (§7.1.1).
    ///
    /// Not [`LocalCandidate::priority`], and that is the whole point — see [`check_priority`].
    #[must_use]
    pub fn check_priority(&self) -> Priority {
        check_priority(self.local_preference, self.gathered.component)
    }
}

/// A remote candidate: one the peer signalled, or one §7.3.1.3 learned from a check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteCandidate {
    /// Its identity, stable for the life of the agent.
    pub id: RemoteId,
    /// Where to send a check.
    pub address: SocketAddr,
    /// How the peer obtained it, or [`CandidateType::PeerReflexive`] when it was learned here.
    pub kind: CandidateType,
    /// Which component it is for.
    pub component: ComponentId,
    /// Its foundation.
    pub foundation: RemoteFoundation,
    /// Its priority, as the peer computed it. Range-checked to RFC 8839 §5.1's `1..=2^31−1` by
    /// [`Priority`] itself — which is what keeps §6.1.2.3's pair-priority arithmetic inside a
    /// `u64`, so nothing here re-checks or widens it.
    pub priority: Priority,
}

impl RemoteCandidate {
    /// The remote candidate an `a=candidate` line describes.
    ///
    /// `None` when the line names a transport sipx does not check over: RFC 8839 §5.1's grammar
    /// admits a `transport-extension`, and [spec] §3 says such a line is accepted and discarded
    /// rather than failing the description a usable candidate arrived in.
    ///
    /// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
    #[must_use]
    pub fn signalled(id: RemoteId, candidate: &Candidate) -> Option<Self> {
        if candidate.transport != Transport::Udp {
            return None;
        }
        Some(Self {
            id,
            address: SocketAddr::new(candidate.address, candidate.port),
            kind: candidate.kind,
            component: candidate.component,
            foundation: RemoteFoundation::Signalled(candidate.foundation.clone()),
            priority: candidate.priority,
        })
    }
}

/// §5.1.2.2's recommended type preference for a candidate of this type ([spec] §4's table).
///
/// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
#[must_use]
pub const fn type_preference(kind: CandidateType) -> u8 {
    match kind {
        CandidateType::Host => HOST_PREFERENCE,
        CandidateType::PeerReflexive => PEER_REFLEXIVE_PREFERENCE,
        CandidateType::ServerReflexive => SERVER_REFLEXIVE_PREFERENCE,
        CandidateType::Relayed => RELAYED_PREFERENCE,
    }
}

/// §5.1.2.1's formula, exactly as it is printed:
///
/// ```text
/// priority = (2^24)*(type preference) +
///            (2^8)*(local preference) +
///            (2^0)*(256 - component ID)
/// ```
///
/// The ordering this produces is the only thing that makes two independent implementations agree
/// on which pair wins, so it is written once and every caller goes through it.
///
/// `type_preference` is held to §5.1.2.1's `0..=126`; a larger value would put the result past
/// 2^31 − 1, which [`Priority`] does not hold. The result is clamped up to [`Priority::MIN`] for
/// the one input that yields zero — a relayed candidate (preference 0) that is also the 65536th
/// of its type (local preference 0) for component 256 — because §5.1.2 requires a priority to be
/// "a positive integer".
#[must_use]
pub fn priority(type_preference: u8, local_preference: u16, component: ComponentId) -> Priority {
    let type_preference = u32::from(type_preference.min(MAX_TYPE_PREFERENCE));
    let raw = (type_preference << 24)
        + (u32::from(local_preference) << 8)
        + (256u32.saturating_sub(u32::from(component.get())));
    Priority::new(raw).unwrap_or(Priority::MIN)
}

/// The `PRIORITY` a connectivity check carries (§7.1.1).
///
/// The same formula, "but with the candidate type preference of peer-reflexive candidates" —
/// 110, whatever the candidate sending the check actually is.
///
/// It has to be. That is the priority the *peer* will assign to the peer-reflexive candidate it
/// may learn from this very check (§7.3.1.3 takes it straight out of the attribute), and the two
/// ends have to agree on it. Send the candidate's own priority here and the far end prioritises
/// the candidate it learned from us differently from the way we prioritise it, and the two
/// checklists diverge — which shows up as ICE picking different pairs at the two ends, not as
/// anything that looks like a bug in this function.
#[must_use]
pub fn check_priority(local_preference: u16, component: ComponentId) -> Priority {
    priority(PEER_REFLEXIVE_PREFERENCE, local_preference, component)
}

/// §6.1.2.3's pair priority, with `controlling` the priority of the controlling agent's candidate
/// and `controlled` the controlled agent's:
///
/// ```text
/// pair priority = 2^32*MIN(G,D) + 2*MAX(G,D) + (G>D?1:0)
/// ```
///
/// It fits in a `u64` because [`Priority`] is bounded at 2^31 − 1: the largest value two in-range
/// priorities produce is 2^63 − 2, at `G = D = 2^31 − 1`, where the `G>D` term is zero. That bound
/// is [spec] §6.2's, and the range check that supplies it lives in `sipx-sdp`, on parse — an
/// unchecked ten-digit priority from a peer overflows this expression and silently reorders the
/// checklist that computing it exists to order.
///
/// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
#[must_use]
pub fn pair_priority(controlling: Priority, controlled: Priority) -> u64 {
    let g = u64::from(controlling.get());
    let d = u64::from(controlled.get());
    (1u64 << 32) * g.min(d) + 2 * g.max(d) + u64::from(g > d)
}

/// The distinct tuple §5.1.1.3 makes a foundation out of.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FoundationKey {
    kind: CandidateType,
    /// "Their bases have the same IP address (the ports can be different)" — so the port is not
    /// in the key, and leaving it in would give every socket its own foundation and unfreeze
    /// every pair at once.
    base_ip: IpAddr,
    /// "For reflexive and relayed candidates, the STUN or TURN servers used to obtain them have
    /// the same IP address."
    server: Option<IpAddr>,
    /// "They were obtained using the same transport protocol." One value today; in the key
    /// because §5.1.1.3 puts it there and a second transport would otherwise share foundations
    /// with the first.
    transport: Transport,
}

/// Allocates foundations over the candidates as they are gathered (§5.1.1.3).
#[derive(Debug, Default)]
pub struct Foundations {
    keys: Vec<FoundationKey>,
    learned: u32,
}

impl Foundations {
    /// The foundation for a gathered candidate: the counter already allocated to its tuple, or
    /// the next one.
    pub fn assign(&mut self, candidate: &Gathered, transport: Transport) -> LocalFoundation {
        let key = FoundationKey {
            kind: candidate.kind,
            base_ip: candidate.base_address.ip(),
            server: candidate.server,
            transport,
        };
        let existing = self
            .keys
            .iter()
            .position(|known| *known == key)
            .unwrap_or_else(|| {
                self.keys.push(key);
                self.keys.len().saturating_sub(1)
            });
        LocalFoundation(
            u32::try_from(existing)
                .unwrap_or(u32::MAX)
                .saturating_add(1),
        )
    }

    /// The next foundation for a peer-reflexive *remote* candidate (§7.3.1.3): "an arbitrary
    /// value, different from the foundations of all other remote candidates".
    pub fn learn_remote(&mut self) -> RemoteFoundation {
        self.learned = self.learned.saturating_add(1);
        RemoteFoundation::Learned(self.learned)
    }
}

/// Assign §5.1.2.1's local preferences across a gathered set, and price every candidate.
///
/// 65535 when a candidate is the only one of its type for its component; otherwise 65535, 65534,
/// … descending over the candidates **sorted by address bytes**, which is the whole reason this
/// is a function over the set rather than a field on a candidate. §5.1.2.1 requires the value to
/// be unique per type and component; ordering by whatever the OS enumerated first would make the
/// same host produce different priorities on different runs, and the priorities are what the far
/// end reasons about.
pub fn assign_local_preferences(candidates: &mut [LocalCandidate]) {
    let mut ordered: Vec<usize> = (0..candidates.len()).collect();
    ordered.sort_by_key(|index| {
        candidates.get(*index).map(|candidate| {
            (
                type_preference(candidate.gathered.kind),
                candidate.gathered.component.get(),
                candidate.gathered.address.ip(),
                candidate.gathered.address.port(),
            )
        })
    });

    let mut previous: Option<(u8, u16)> = None;
    let mut preference = SINGLE_ADDRESS_PREFERENCE;
    for index in ordered {
        let Some(candidate) = candidates.get_mut(index) else {
            continue;
        };
        let group = (
            type_preference(candidate.gathered.kind),
            candidate.gathered.component.get(),
        );
        if previous == Some(group) {
            preference = preference.saturating_sub(1);
        } else {
            preference = SINGLE_ADDRESS_PREFERENCE;
            previous = Some(group);
        }
        candidate.local_preference = preference;
        candidate.priority = priority(
            type_preference(candidate.gathered.kind),
            preference,
            candidate.gathered.component,
        );
    }
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
    use super::*;

    fn component(id: u16) -> ComponentId {
        ComponentId::new(id).unwrap()
    }

    /// [spec] §4's worked vector, three candidates and three stated integers — asserted against
    /// the numbers, not against the formula re-typed into the test. The third is the one RFC 8839
    /// §5.1 prints in its own example line.
    ///
    /// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
    #[test]
    fn the_priority_formula_reproduces_the_specs_three_row_table() {
        assert_eq!(
            priority(HOST_PREFERENCE, SINGLE_ADDRESS_PREFERENCE, component(1)).get(),
            2_130_706_431
        );
        assert_eq!(
            priority(HOST_PREFERENCE, SINGLE_ADDRESS_PREFERENCE, component(2)).get(),
            2_130_706_430
        );
        assert_eq!(
            priority(
                SERVER_REFLEXIVE_PREFERENCE,
                SINGLE_ADDRESS_PREFERENCE,
                component(1)
            )
            .get(),
            1_694_498_815
        );
    }

    /// RFC 8839 §5.1's own example line carries the third row's number, and the candidate that
    /// line describes is a server-reflexive RTP candidate with a single address.
    #[test]
    fn rfc8839s_example_line_carries_the_priority_this_formula_computes() {
        let line = "2 1 UDP 1694498815 192.0.2.3 45664 typ srflx raddr 203.0.113.141 rport 8998";
        let candidate = Candidate::parse(line).unwrap();
        assert_eq!(
            candidate.priority,
            priority(
                SERVER_REFLEXIVE_PREFERENCE,
                SINGLE_ADDRESS_PREFERENCE,
                candidate.component
            )
        );
    }

    /// §5.1.2.1: the peer-reflexive preference MUST be higher than the server-reflexive one,
    /// which is the only ordering constraint the RFC puts on the four values.
    #[test]
    fn the_type_preferences_are_ordered_the_way_the_rfc_requires() {
        let preferences: Vec<u8> = [
            CandidateType::Host,
            CandidateType::PeerReflexive,
            CandidateType::ServerReflexive,
            CandidateType::Relayed,
        ]
        .into_iter()
        .map(type_preference)
        .collect();
        let mut descending = preferences.clone();
        descending.sort_unstable_by(|left, right| right.cmp(left));
        descending.dedup();
        assert_eq!(preferences, descending);
        assert_eq!(type_preference(CandidateType::Host), HOST_PREFERENCE);
        assert_eq!(
            type_preference(CandidateType::PeerReflexive),
            PEER_REFLEXIVE_PREFERENCE
        );
    }

    /// §7.1.1: a check's `PRIORITY` is computed with the peer-reflexive type preference whatever
    /// the candidate is. A host candidate's own priority is 2130706431; the check it sends says
    /// 1862270975.
    #[test]
    fn a_check_carries_the_peer_reflexive_priority_and_not_the_candidates_own() {
        let host = priority(HOST_PREFERENCE, SINGLE_ADDRESS_PREFERENCE, component(1));
        let check = check_priority(SINGLE_ADDRESS_PREFERENCE, component(1));
        assert_eq!(host.get(), 2_130_706_431);
        assert_eq!(check.get(), 1_862_270_975);
        assert_ne!(host, check);
        assert_eq!(
            check,
            priority(
                PEER_REFLEXIVE_PREFERENCE,
                SINGLE_ADDRESS_PREFERENCE,
                component(1)
            )
        );
    }

    /// A relayed candidate sends a check that claims 110, not 0 — the case where getting §7.1.1
    /// wrong is most visible, because the two priorities are furthest apart.
    #[test]
    fn even_a_relayed_candidate_claims_the_peer_reflexive_preference_in_a_check() {
        let relayed = priority(RELAYED_PREFERENCE, SINGLE_ADDRESS_PREFERENCE, component(1));
        assert_eq!(relayed.get(), 16_777_215);
        assert_eq!(
            check_priority(SINGLE_ADDRESS_PREFERENCE, component(1)).get(),
            1_862_270_975
        );
    }

    /// [spec] §6.2's arithmetic: the largest pair priority two in-range priorities produce is
    /// 2^63 − 2, at `G = D`, where the `G>D` term is zero.
    ///
    /// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
    #[test]
    fn the_pair_priority_is_the_rfcs_expression_and_stays_inside_a_u64() {
        let max = Priority::MAX;
        assert_eq!(pair_priority(max, max), (1u64 << 63) - 2);

        let g = Priority::new(2_130_706_431).unwrap();
        let d = Priority::new(1_694_498_815).unwrap();
        let expected = (1u64 << 32) * u64::from(d.get()) + 2 * u64::from(g.get()) + 1;
        assert_eq!(pair_priority(g, d), expected);
        // The last term is the tie-break, and it is asymmetric on purpose: the same two
        // candidates in the same roles must give both ends the same number.
        assert_eq!(pair_priority(d, g), expected - 1);
    }

    #[test]
    fn foundations_are_equal_exactly_when_section_5_1_1_3_says_they_are() {
        let mut foundations = Foundations::default();
        let host = |ip: &str, port: u16| Gathered {
            base: LocalBase(0),
            base_address: SocketAddr::new(ip.parse().unwrap(), port),
            address: SocketAddr::new(ip.parse().unwrap(), port),
            kind: CandidateType::Host,
            component: component(1),
            server: None,
        };

        let media = foundations.assign(&host("192.0.2.1", 5000), Transport::Udp);
        // Same base IP, different port: "the ports can be different".
        let control = foundations.assign(&host("192.0.2.1", 5001), Transport::Udp);
        assert_eq!(media, control);

        // A different base IP is a different foundation.
        let elsewhere = foundations.assign(&host("192.0.2.2", 5000), Transport::Udp);
        assert_ne!(media, elsewhere);

        // Same base, different type: different foundation.
        let mut reflexive = host("192.0.2.1", 5000);
        reflexive.kind = CandidateType::ServerReflexive;
        reflexive.server = Some("198.51.100.1".parse().unwrap());
        let srflx = foundations.assign(&reflexive, Transport::Udp);
        assert_ne!(media, srflx);

        // Same type and base, a different STUN server: different foundation.
        let mut second_server = reflexive;
        second_server.server = Some("198.51.100.2".parse().unwrap());
        assert_ne!(srflx, foundations.assign(&second_server, Transport::Udp));

        // And the same tuple twice is the same foundation, which is the property §6.1.2.6 uses.
        assert_eq!(srflx, foundations.assign(&reflexive, Transport::Udp));
    }

    /// §7.3.1.3's foundation for a learned remote candidate is "different from the foundations of
    /// all other remote candidates" — including every foundation a peer could have signalled.
    #[test]
    fn a_learned_remote_foundation_collides_with_nothing() {
        let mut foundations = Foundations::default();
        let first = foundations.learn_remote();
        let second = foundations.learn_remote();
        assert_ne!(first, second);
        assert_ne!(
            first,
            RemoteFoundation::Signalled(Foundation::new("1").unwrap())
        );
    }

    #[test]
    fn local_preferences_descend_over_addresses_sorted_by_bytes() {
        let gathered = |ip: &str| Gathered {
            base: LocalBase(0),
            base_address: SocketAddr::new(ip.parse().unwrap(), 5000),
            address: SocketAddr::new(ip.parse().unwrap(), 5000),
            kind: CandidateType::Host,
            component: component(1),
            server: None,
        };
        let candidate = |ip: &str| LocalCandidate {
            id: LocalId(0),
            gathered: gathered(ip),
            foundation: LocalFoundation(1),
            local_preference: 0,
            priority: Priority::MIN,
        };

        // Handed over in an order no interface enumeration would guarantee.
        let mut candidates = vec![
            candidate("192.0.2.9"),
            candidate("192.0.2.1"),
            candidate("192.0.2.5"),
        ];
        assign_local_preferences(&mut candidates);

        let preference = |ip: &str| {
            candidates
                .iter()
                .find(|candidate| candidate.gathered.address.ip().to_string() == ip)
                .unwrap()
                .local_preference
        };
        assert_eq!(preference("192.0.2.1"), 65535);
        assert_eq!(preference("192.0.2.5"), 65534);
        assert_eq!(preference("192.0.2.9"), 65533);
    }

    /// One candidate of a type for a component gets §5.1.2.1's SHOULD value, and a second
    /// component starts again from it — the uniqueness rule is per type *and* component.
    #[test]
    fn a_single_address_gets_65535_for_every_component() {
        let gathered = |component_id: u16| Gathered {
            base: LocalBase(0),
            base_address: SocketAddr::new("192.0.2.1".parse().unwrap(), 5000 + component_id),
            address: SocketAddr::new("192.0.2.1".parse().unwrap(), 5000 + component_id),
            kind: CandidateType::Host,
            component: component(component_id),
            server: None,
        };
        let mut candidates = vec![
            LocalCandidate {
                id: LocalId(1),
                gathered: gathered(1),
                foundation: LocalFoundation(1),
                local_preference: 0,
                priority: Priority::MIN,
            },
            LocalCandidate {
                id: LocalId(2),
                gathered: gathered(2),
                foundation: LocalFoundation(1),
                local_preference: 0,
                priority: Priority::MIN,
            },
        ];
        assign_local_preferences(&mut candidates);
        assert_eq!(candidates[0].local_preference, SINGLE_ADDRESS_PREFERENCE);
        assert_eq!(candidates[1].local_preference, SINGLE_ADDRESS_PREFERENCE);
        assert_eq!(candidates[0].priority.get(), 2_130_706_431);
        assert_eq!(candidates[1].priority.get(), 2_130_706_430);
    }
}
