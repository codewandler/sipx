//! Gathering local candidates, and the description they make (RFC 8445 §5.1.1, RFC 8839 §5.1,
//! §13.2; [spec] §5, §15).
//!
//! Two kinds of candidate, and both come from sockets that are already bound. A **host**
//! candidate is what [`MediaPort`](crate::MediaPort) got from the OS. A **server-reflexive** one
//! is what a STUN server says it sees, obtained over
//! [`sipx_transport::stun`](https://docs.rs/sipx-transport) — the Binding client that already
//! exists, because [spec] §15 says a second one would be a second thing to get wrong.
//!
//! The candidates are *priced* by the agent and not here. §5.1.1.3's foundation and §5.1.2.1's
//! local preference are properties of the whole gathered set — "MUST be unique for each" candidate
//! of a type — and are not facts any single candidate knows about itself, so what this module
//! produces is a stream of [`Gathered`] and what it reads back is
//! [`Agent::local_candidates`](super::Agent::local_candidates).
//!
//! [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::net::UdpSocket;

use sipx_sdp::ice::{
    Candidate, CandidateType, ComponentId, Credentials, Foundation, RelatedAddress, Transport,
};

use super::agent::{Agent, Config, Input, Output};
use super::candidate::{Gathered, LocalBase, LocalCandidate};
use super::negotiate::Negotiation;
use crate::counters::DiscardMeters;

/// How long to wait for one Binding Response before trying again (RFC 5389 §7.2.1's initial RTO).
const STUN_RTO: Duration = Duration::from_millis(500);

/// Everything gathering needs that the sockets do not already supply.
#[derive(Debug, Clone)]
pub struct Gathering {
    /// Our short-term credentials for this ICE session (RFC 8839 §5.4), which go in the offer or
    /// the answer and key every check in both directions ([spec] §11.2).
    ///
    /// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
    pub credentials: Credentials,
    /// Whether sipx sent the initial offer — RFC 8445 §6.1.1's role determination between two
    /// full agents.
    pub offerer: bool,
    /// §7.1.3's 64-bit tiebreaker, drawn once per ICE session.
    pub tiebreaker: u64,
    /// The STUN server to ask for a server-reflexive candidate (§5.1.1.2). `None` gathers host
    /// candidates alone, which is the right answer on a network with no NAT and the only answer
    /// when no server is configured.
    pub stun_server: Option<SocketAddr>,
    /// How long to keep asking it before giving up.
    ///
    /// Gathering that never finishes is an offer that never goes out, so this is a deadline and
    /// not a retry count: whatever has been gathered when it expires is what is offered, and a
    /// STUN server that is down costs one call setup this long and nothing else.
    pub stun_timeout: Duration,
    /// The agent's own configuration — §14's timers and §6.1.2.5's pair limit.
    pub agent: Config,
}

impl Gathering {
    /// Gather host candidates only, with a fresh tiebreaker.
    ///
    /// The tiebreaker is drawn here rather than taken because §7.1.3 wants a random one per ICE
    /// session and a caller that has no opinion should not have to have one; a caller that does —
    /// a test walking §7.3.1.1's `T = V` row — sets the field afterwards.
    #[must_use]
    pub fn new(credentials: Credentials, offerer: bool) -> Self {
        Self {
            credentials,
            offerer,
            tiebreaker: rand::random(),
            stun_server: None,
            stun_timeout: Duration::from_secs(2),
            agent: Config::default(),
        }
    }
}

/// One bound socket, and which component and base it is.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Base<'a> {
    /// The index the agent will name this socket by ([spec] §2).
    ///
    /// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
    pub index: LocalBase,
    /// Which component of the stream it carries.
    pub component: ComponentId,
    /// The socket itself, still exclusively ours: gathering runs before any receive loop does.
    pub socket: &'a UdpSocket,
}

/// What sipx will put in its own description, and the agent that will drive it.
///
/// Held by the caller across offer/answer: it is built when the port is bound and the offer is
/// written, and it is consumed when the session starts. The agent inside it has already been told
/// what was gathered, so the only thing still missing is the peer's half.
#[derive(Debug)]
pub struct LocalDescription {
    agent: Agent,
    /// Outputs the agent produced before there was a driver to perform them.
    ///
    /// There are none today — forming checklists arms Ta and sends nothing — but they are carried
    /// rather than dropped, because "the agent emitted something and nobody did it" is the one
    /// failure this type could introduce silently.
    pending: Vec<Output>,
    credentials: Credentials,
    candidates: Vec<Candidate>,
    defaults: Vec<(ComponentId, SocketAddr)>,
}

impl LocalDescription {
    /// Our credentials, for `a=ice-ufrag` and `a=ice-pwd`.
    #[must_use]
    pub const fn credentials(&self) -> &Credentials {
        &self.credentials
    }

    /// The gathered candidates, priced, in descending priority.
    #[must_use]
    pub fn candidates(&self) -> &[Candidate] {
        &self.candidates
    }

    /// Where the peer should send this component if it turns out not to do ICE — the `c=`/`m=`
    /// default destination (RFC 8839 §4.2.1; [spec] §13.2).
    ///
    /// "The candidate sipx would use if the peer turned out not to do ICE", which for a full agent
    /// is the highest-priority one for the component.
    ///
    /// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
    #[must_use]
    pub fn default_destination(&self, component: ComponentId) -> Option<SocketAddr> {
        self.defaults
            .iter()
            .find(|(id, _)| *id == component)
            .map(|(_, address)| *address)
    }

    /// The media-level attributes for the offer or the answer (RFC 8839 §4.2.1, §4.2.2).
    ///
    /// `a=ice-options:ice2` goes out on both: [spec] §8 makes aggressive nomination unavailable
    /// rather than optional, and `ice2` is how a peer is told that no pair will be re-nominated
    /// mid-session.
    ///
    /// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
    #[must_use]
    pub fn attributes(&self) -> Vec<sipx_sdp::Attribute> {
        let mut attributes = vec![
            sipx_sdp::Attribute::valued("ice-ufrag", self.credentials.ufrag()),
            sipx_sdp::Attribute::valued("ice-pwd", self.credentials.pwd()),
            sipx_sdp::Attribute::valued("ice-options", sipx_sdp::ice::ICE2),
        ];
        attributes.extend(
            self.candidates
                .iter()
                .map(|candidate| sipx_sdp::Attribute::valued("candidate", candidate.to_value())),
        );
        attributes
    }

    /// Feed the agent the peer's half of the exchange.
    ///
    /// Returns whether ICE will actually be driven for this stream. `false` for
    /// [`Negotiation::Absent`] and [`Negotiation::Mismatch`] alike — RFC 8839 §5.3 says ICE MUST
    /// NOT be used for a mismatched stream, so the two differ in what the *answer* says and not in
    /// what the media port does.
    pub fn accept(&mut self, negotiation: &Negotiation) -> bool {
        let Negotiation::Ice {
            credentials,
            candidates,
            lite,
        } = negotiation
        else {
            return false;
        };
        self.pending
            .extend(self.agent.handle(Input::RemoteDescription {
                credentials: credentials.clone(),
                candidates: candidates.clone(),
                lite: *lite,
            }));
        true
    }

    /// Whether [`Self::accept`] has been given a peer description ICE can run against.
    #[must_use]
    pub(crate) fn running(&self) -> bool {
        !self.agent.remote_candidates().is_empty()
    }

    /// Take the agent and whatever it has already asked for, for the driver to run.
    pub(crate) fn into_driver_parts(self) -> (Agent, Vec<Output>) {
        (self.agent, self.pending)
    }
}

/// Gather over these sockets and build the description they make (§5.1.1).
///
/// Every socket contributes a host candidate; the first one contributes a server-reflexive
/// candidate too when a STUN server is configured and answers. `GatheringDone` is fed at the end,
/// which is what lets the agent form checklists as soon as the peer's half arrives.
pub(crate) async fn gather(
    bases: &[Base<'_>],
    config: &Gathering,
    discards: Arc<DiscardMeters>,
) -> LocalDescription {
    let mut agent = Agent::new(
        config.agent,
        config.offerer,
        config.credentials.clone(),
        config.tiebreaker,
    );
    let mut pending = Vec::new();

    for base in bases {
        let Ok(address) = base.socket.local_addr() else {
            continue;
        };
        if address.ip().is_unspecified() {
            // A wildcard bind has no host candidate: `0.0.0.0` is not somewhere a peer can send,
            // and offering it would advertise a path that cannot work while hiding that no usable
            // one was found. Enumerating the interfaces behind a wildcard bind is what a
            // gathering agent would do instead, and it is not something this crate can do without
            // a platform dependency it does not have.
            tracing::debug!(%address, "no host candidate for a wildcard bind");
            continue;
        }
        pending.extend(agent.handle(Input::LocalCandidate(Gathered {
            base: base.index,
            base_address: address,
            address,
            kind: CandidateType::Host,
            component: base.component,
            server: None,
        })));

        let Some(server) = config.stun_server else {
            continue;
        };
        let Some(mapped) = reflexive(base.socket, server, config.stun_timeout, &discards).await
        else {
            continue;
        };
        if mapped == address {
            // §5.1.3: a server-reflexive candidate whose address is one of our host candidates is
            // redundant and is discarded. On a network with no NAT that is every one of them.
            discards
                .ice_redundant_candidates
                .fetch_add(1, Ordering::Relaxed);
            tracing::debug!(%address, "no nat: the reflexive candidate is the host one");
            continue;
        }
        pending.extend(agent.handle(Input::LocalCandidate(Gathered {
            base: base.index,
            base_address: address,
            address: mapped,
            kind: CandidateType::ServerReflexive,
            component: base.component,
            server: Some(server.ip()),
        })));
    }

    pending.extend(agent.handle(Input::GatheringDone));

    let candidates = lines(agent.local_candidates());
    let defaults = defaults(&candidates);
    LocalDescription {
        agent,
        pending,
        credentials: config.credentials.clone(),
        candidates,
        defaults,
    }
}

/// Ask a STUN server what address it sees, over [`sipx_transport::stun`] (§5.1.1.2).
///
/// The socket is the one the candidate is for, because that is what makes the answer a candidate:
/// the mapping a NAT holds is per source address and port, so a reflexive address learned on any
/// other socket describes a path media will never take.
///
/// Retransmission is RFC 5389 §7.2.1's, truncated at the deadline rather than at Rc: gathering is
/// on the call-setup path, and an offer that waits out the full ladder for a server that is down
/// is an offer nobody sends.
async fn reflexive(
    socket: &UdpSocket,
    server: SocketAddr,
    within: Duration,
    discards: &DiscardMeters,
) -> Option<SocketAddr> {
    let id = sipx_transport::stun::new_transaction_id();
    let request = sipx_transport::stun::binding_request(&id);
    let deadline = tokio::time::Instant::now().checked_add(within)?;
    let mut rto = STUN_RTO;
    let mut datagram = vec![0u8; 1500];

    while tokio::time::Instant::now() < deadline {
        if socket.send_to(&request, server).await.is_err() {
            return None;
        }
        let wait = deadline
            .saturating_duration_since(tokio::time::Instant::now())
            .min(rto);
        let until = tokio::time::Instant::now().checked_add(wait)?;
        loop {
            let read = tokio::time::timeout_at(until, socket.recv_from(&mut datagram)).await;
            let Ok(Ok((len, from))) = read else {
                break;
            };
            if from != server {
                // Something else on the media port. It is not this transaction's business, and
                // gathering runs before there is anywhere to hand it, so it is dropped.
                discards
                    .ice_gathering_foreign_datagrams
                    .fetch_add(1, Ordering::Relaxed);
                tracing::debug!(%from, %server, "dropping a datagram from outside the STUN gathering transaction");
                continue;
            }
            let Some(reply) = sipx_transport::stun::parse_reply(datagram.get(..len)?) else {
                continue;
            };
            if reply.id() != id {
                continue;
            }
            return match reply {
                sipx_transport::stun::Reply::Bound { mapped, .. } => mapped,
                // §4.4.2 of RFC 5626 calls an error response a failed flow; here it is simply no
                // reflexive candidate, and the host ones still stand.
                sipx_transport::stun::Reply::Failed { .. } => None,
            };
        }
        rto = rto.saturating_mul(2);
    }
    None
}

/// Turn the agent's priced candidates into `a=candidate` lines (RFC 8839 §5.1).
///
/// Shared with the driver, which signals the same list again for every later exchange on the call
/// ([spec] §13.5) — one ordering rule and one set of `raddr`/`rport` decisions, so a re-offer
/// cannot describe the same sockets differently from the offer that opened the session.
///
/// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
pub(crate) fn lines(candidates: &[LocalCandidate]) -> Vec<Candidate> {
    let mut lines: Vec<Candidate> = candidates
        .iter()
        .filter_map(|candidate| {
            Some(Candidate {
                foundation: Foundation::new(&candidate.foundation.0.to_string())?,
                component: candidate.gathered.component,
                transport: Transport::Udp,
                priority: candidate.priority,
                address: candidate.gathered.address.ip(),
                port: candidate.gathered.address.port(),
                kind: candidate.gathered.kind,
                // §5.1: `raddr`/`rport` MUST be present for a reflexive candidate and MUST be
                // absent for a host one. The base is what they name.
                related: related(candidate),
                extensions: Vec::new(),
            })
        })
        .collect();
    // Descending priority, so the first line for a component is also the default destination and
    // a reader of the offer sees them in the order ICE will reason about them.
    lines.sort_by(|left, right| {
        right
            .priority
            .get()
            .cmp(&left.priority.get())
            .then_with(|| left.component.get().cmp(&right.component.get()))
    });
    lines
}

/// The `raddr`/`rport` a candidate of this type carries (RFC 8839 §5.1).
fn related(candidate: &LocalCandidate) -> Option<RelatedAddress> {
    match candidate.gathered.kind {
        CandidateType::Host => None,
        CandidateType::ServerReflexive | CandidateType::PeerReflexive | CandidateType::Relayed => {
            Some(RelatedAddress {
                address: candidate.gathered.base_address.ip(),
                port: candidate.gathered.base_address.port(),
            })
        }
    }
}

/// The highest-priority candidate for each component ([spec] §13.2).
///
/// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
fn defaults(candidates: &[Candidate]) -> Vec<(ComponentId, SocketAddr)> {
    let mut defaults: Vec<(ComponentId, SocketAddr)> = Vec::new();
    for candidate in candidates {
        if defaults.iter().any(|(id, _)| *id == candidate.component) {
            continue;
        }
        defaults.push((
            candidate.component,
            SocketAddr::new(candidate.address, candidate.port),
        ));
    }
    defaults
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

    fn credentials() -> Credentials {
        Credentials::new("8hhY", "asd88fgpdd777uzjYhagZg").expect("valid")
    }

    async fn bound() -> UdpSocket {
        UdpSocket::bind("127.0.0.1:0".parse::<SocketAddr>().unwrap())
            .await
            .expect("a loopback port")
    }

    /// The two components come off the two sockets [`MediaPort`](crate::MediaPort) binds, and the
    /// priorities are §4's table: host, single address, RTP and RTCP.
    #[tokio::test]
    async fn host_candidates_come_off_the_bound_sockets() {
        let (media, control) = (bound().await, bound().await);
        let description = gather(
            &[
                Base {
                    index: LocalBase(0),
                    component: ComponentId::RTP,
                    socket: &media,
                },
                Base {
                    index: LocalBase(1),
                    component: ComponentId::RTCP,
                    socket: &control,
                },
            ],
            &Gathering::new(credentials(), true),
            Arc::new(DiscardMeters::default()),
        )
        .await;

        assert_eq!(description.candidates().len(), 2);
        let first = &description.candidates()[0];
        assert_eq!(first.component, ComponentId::RTP);
        assert_eq!(first.kind, CandidateType::Host);
        assert_eq!(first.priority.get(), 2_130_706_431);
        assert_eq!(first.related, None, "a host candidate carries no raddr");
        assert_eq!(description.candidates()[1].priority.get(), 2_130_706_430);

        assert_eq!(
            description.default_destination(ComponentId::RTP),
            Some(media.local_addr().unwrap())
        );
        assert_eq!(
            description.default_destination(ComponentId::RTCP),
            Some(control.local_addr().unwrap())
        );
    }

    /// [spec] §6.1: sipx offers component 2 only when the control port was actually obtained.
    /// A driver that offered it anyway would have the peer checking an address nothing is bound
    /// to, and RFC 8445 §6.1.2.2's reduction to the minimum component count is exactly what a
    /// peer does with the offer that does not.
    ///
    /// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
    #[tokio::test]
    async fn no_control_port_means_no_second_component() {
        let rtp = bound().await;
        let description = gather(
            &[Base {
                index: LocalBase(0),
                component: ComponentId::RTP,
                socket: &rtp,
            }],
            &Gathering::new(credentials(), true),
            Arc::new(DiscardMeters::default()),
        )
        .await;

        assert_eq!(description.candidates().len(), 1);
        assert_eq!(description.candidates()[0].component, ComponentId::RTP);
        assert_eq!(description.default_destination(ComponentId::RTCP), None);
    }

    /// A wildcard bind is not a candidate: `0.0.0.0` is nowhere to send to, and advertising it
    /// would hide that nothing usable was gathered behind a line that looks like one.
    #[tokio::test]
    async fn a_wildcard_bind_yields_no_host_candidate() {
        let any = UdpSocket::bind("0.0.0.0:0".parse::<SocketAddr>().unwrap())
            .await
            .expect("bound");
        let description = gather(
            &[Base {
                index: LocalBase(0),
                component: ComponentId::RTP,
                socket: &any,
            }],
            &Gathering::new(credentials(), true),
            Arc::new(DiscardMeters::default()),
        )
        .await;
        assert!(description.candidates().is_empty());
    }

    /// §5.1.1.2, over the Binding client [spec] §15 says to reuse: the address the server reports
    /// becomes a candidate whose base is the socket it was learned on, with `raddr`/`rport`
    /// naming that base as §5.1 requires.
    ///
    /// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
    #[tokio::test]
    async fn a_server_reflexive_candidate_comes_from_the_stun_server() {
        let server = bound().await;
        let server_address = server.local_addr().unwrap();
        let reported: SocketAddr = "198.51.100.7:31337".parse().unwrap();
        tokio::spawn(async move {
            let mut datagram = vec![0u8; 1500];
            let Ok((_len, from)) = server.recv_from(&mut datagram).await else {
                return;
            };
            let id: [u8; 12] = datagram[8..20].try_into().expect("a header");
            let _ = server.send_to(&binding_response(&id, reported), from).await;
        });

        let rtp = bound().await;
        let mut gathering = Gathering::new(credentials(), true);
        gathering.stun_server = Some(server_address);
        let description = gather(
            &[Base {
                index: LocalBase(0),
                component: ComponentId::RTP,
                socket: &rtp,
            }],
            &gathering,
            Arc::new(DiscardMeters::default()),
        )
        .await;

        let reflexive = description
            .candidates()
            .iter()
            .find(|candidate| candidate.kind == CandidateType::ServerReflexive)
            .expect("the server's answer became a candidate");
        assert_eq!(reflexive.address, reported.ip());
        assert_eq!(reflexive.port, reported.port());
        assert_eq!(reflexive.priority.get(), 1_694_498_815);
        let related = reflexive.related.as_ref().expect("srflx carries raddr");
        assert_eq!(related.address, rtp.local_addr().unwrap().ip());
        assert_eq!(related.port, rtp.local_addr().unwrap().port());

        // §13.2's default destination is the highest-priority candidate, which is the host one.
        assert_eq!(
            description.default_destination(ComponentId::RTP),
            Some(rtp.local_addr().unwrap())
        );
    }

    /// A STUN server that never answers costs the deadline and nothing else: the host candidates
    /// are still offered, because an offer that waits for a server that is down is an offer
    /// nobody sends.
    #[tokio::test]
    async fn a_silent_stun_server_still_yields_the_host_candidates() {
        // Bound and never read, so the request arrives and no answer ever comes back.
        let black_hole = bound().await;
        let rtp = bound().await;
        let mut gathering = Gathering::new(credentials(), true);
        gathering.stun_server = Some(black_hole.local_addr().unwrap());
        gathering.stun_timeout = Duration::from_millis(120);

        let description = gather(
            &[Base {
                index: LocalBase(0),
                component: ComponentId::RTP,
                socket: &rtp,
            }],
            &gathering,
            Arc::new(DiscardMeters::default()),
        )
        .await;
        assert_eq!(description.candidates().len(), 1);
        assert_eq!(description.candidates()[0].kind, CandidateType::Host);
    }

    /// RFC 5389 §15.2's `XOR-MAPPED-ADDRESS` in a Binding Response, for the fake server above.
    fn binding_response(id: &[u8; 12], mapped: SocketAddr) -> Vec<u8> {
        let SocketAddr::V4(v4) = mapped else {
            panic!("the fixture is IPv4");
        };
        let cookie = sipx_transport::stun::MAGIC_COOKIE;
        let mut value = vec![0u8, 0x01];
        value.extend_from_slice(&(v4.port() ^ u16::try_from(cookie >> 16).unwrap()).to_be_bytes());
        let octets = u32::from(*v4.ip()) ^ cookie;
        value.extend_from_slice(&octets.to_be_bytes());

        let mut message = vec![0x01, 0x01];
        message.extend_from_slice(&u16::try_from(value.len() + 4).unwrap().to_be_bytes());
        message.extend_from_slice(&cookie.to_be_bytes());
        message.extend_from_slice(id);
        message.extend_from_slice(&0x0020u16.to_be_bytes());
        message.extend_from_slice(&u16::try_from(value.len()).unwrap().to_be_bytes());
        message.extend_from_slice(&value);
        message
    }
}
