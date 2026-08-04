//! Hop-by-hop overload control (RFC 7339 and RFC 7415).
//!
//! The arithmetic takes elapsed time and seeded randomness as inputs. The endpoint driver owns the
//! controller and supplies both, keeping response updates and request admission on its serial loop.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use sipx_sip::headers::{OcParameter, OverloadAlgorithm, OverloadSequence, Via, first_hop_end};
use sipx_sip::{Header, HeaderName, Headers, Request, Response};

/// Which RFC 7339 message category local policy assigns to a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestCategory {
    /// Traffic reduced first.
    Ordinary,
    /// In-dialog, emergency, or other locally important traffic reduced only when necessary.
    Protected,
}

/// The feedback an endpoint reports when its application queue is full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverloadFeedback {
    /// Ask the upstream peer to discard this percentage of requests.
    Loss(u8),
    /// Cap the upstream peer at this many requests per second.
    Rate(u32),
}

/// Endpoint overload-control policy.
#[derive(Debug, Clone)]
pub struct OverloadConfig {
    /// Advertise RFC 7339/7415 client support on every outgoing topmost `Via`.
    ///
    /// Off by default because overload control is an extension negotiated hop by hop. Enabling it
    /// retains the complete `loss,rate` offer; it does not select a compatibility subset per peer.
    pub advertise: bool,
    /// What to report upstream when the application queue is full.
    pub feedback: OverloadFeedback,
    /// How long that report remains valid.
    pub validity: Duration,
    /// RFC 7415's `TAU1` for ordinary requests, in target inter-request intervals.
    pub rate_tolerance_intervals: u32,
    /// RFC 7415's larger `TAU2` for protected requests, in target intervals.
    pub rate_priority_tolerance_intervals: u32,
    /// Most downstream peers whose feedback sequence and algorithm state are retained.
    pub peer_limit: usize,
    /// Assign a request to one of RFC 7339 §7.2's two categories.
    pub categorize: fn(&Request) -> RequestCategory,
}

impl Default for OverloadConfig {
    fn default() -> Self {
        Self {
            advertise: false,
            feedback: OverloadFeedback::Loss(100),
            validity: Duration::from_millis(500),
            rate_tolerance_intervals: 5,
            rate_priority_tolerance_intervals: 10,
            peer_limit: 1024,
            categorize: |_| RequestCategory::Ordinary,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Report {
    algorithm: OverloadAlgorithm,
    value: u32,
    validity: Option<Duration>,
    sequence: OverloadSequence,
}

#[derive(Debug)]
enum Active {
    Loss {
        percentage: f64,
        ordinary: u32,
        protected: u32,
    },
    Rate(RateState),
}

#[derive(Debug)]
struct PeerState {
    sequence: OverloadSequence,
    until: Option<Duration>,
    active: Option<Active>,
    last_used: Duration,
}

#[derive(Debug)]
struct RateState {
    interval: f64,
    ordinary_tolerance: f64,
    protected_tolerance: f64,
    content: f64,
    last_forwarded: Duration,
}

impl RateState {
    fn new(
        rate: u32,
        tolerance_intervals: u32,
        priority_tolerance_intervals: u32,
        now: Duration,
    ) -> Self {
        if rate == 0 {
            return Self {
                interval: f64::INFINITY,
                ordinary_tolerance: 0.0,
                protected_tolerance: 0.0,
                content: 0.0,
                last_forwarded: now,
            };
        }
        let interval = 1.0 / f64::from(rate);
        Self {
            interval,
            ordinary_tolerance: interval * f64::from(tolerance_intervals),
            protected_tolerance: interval * f64::from(priority_tolerance_intervals),
            content: 0.0,
            last_forwarded: now,
        }
    }

    fn admit(&mut self, now: Duration, category: RequestCategory) -> bool {
        if self.interval.is_infinite() {
            return false;
        }
        let elapsed = now.saturating_sub(self.last_forwarded).as_secs_f64();
        let provisional = (self.content - elapsed).max(0.0);
        let tolerance = match category {
            RequestCategory::Ordinary => self.ordinary_tolerance,
            RequestCategory::Protected => self.protected_tolerance,
        };
        if provisional > tolerance {
            return false;
        }
        self.content = provisional + self.interval;
        self.last_forwarded = now;
        true
    }
}

/// Per-next-hop client state. Owned and called only by the endpoint driver.
#[derive(Debug)]
pub(crate) struct Controller {
    peers: HashMap<SocketAddr, PeerState>,
    random: StdRng,
    tolerance_intervals: u32,
    priority_tolerance_intervals: u32,
    peer_limit: usize,
}

impl Controller {
    pub(crate) fn new(
        tolerance_intervals: u32,
        priority_tolerance_intervals: u32,
        peer_limit: usize,
    ) -> Self {
        Self {
            peers: HashMap::new(),
            random: StdRng::from_os_rng(),
            tolerance_intervals,
            priority_tolerance_intervals,
            peer_limit,
        }
    }

    #[cfg(test)]
    fn seeded(seed: u64, tolerance_intervals: u32, priority_tolerance_intervals: u32) -> Self {
        Self {
            peers: HashMap::new(),
            random: StdRng::seed_from_u64(seed),
            tolerance_intervals,
            priority_tolerance_intervals,
            peer_limit: 1024,
        }
    }

    #[cfg(test)]
    fn seeded_with_limit(
        seed: u64,
        tolerance_intervals: u32,
        priority_tolerance_intervals: u32,
        peer_limit: usize,
    ) -> Self {
        Self {
            peers: HashMap::new(),
            random: StdRng::seed_from_u64(seed),
            tolerance_intervals,
            priority_tolerance_intervals,
            peer_limit,
        }
    }

    pub(crate) fn observe(&mut self, peer: SocketAddr, response: &Response, now: Duration) {
        if let Some(report) = report_from(response) {
            self.apply(peer, &report, now);
        }
    }

    fn apply(&mut self, peer: SocketAddr, report: &Report, now: Duration) {
        if self
            .peers
            .get(&peer)
            .is_some_and(|state| report.sequence <= state.sequence)
        {
            return;
        }

        self.make_room_for(peer, now);

        let validity = report.validity.unwrap_or(Duration::from_millis(500));
        if validity.is_zero() {
            self.peers.insert(
                peer,
                PeerState {
                    sequence: report.sequence,
                    until: None,
                    active: None,
                    last_used: now,
                },
            );
            return;
        }

        let active = match &report.algorithm {
            OverloadAlgorithm::Loss if report.value <= 100 => Some(Active::Loss {
                percentage: f64::from(report.value),
                ordinary: 0,
                protected: 0,
            }),
            OverloadAlgorithm::Rate => Some(Active::Rate(RateState::new(
                report.value,
                self.tolerance_intervals,
                self.priority_tolerance_intervals,
                now,
            ))),
            OverloadAlgorithm::Loss | OverloadAlgorithm::Other(_) => None,
        };
        self.peers.insert(
            peer,
            PeerState {
                sequence: report.sequence,
                until: Some(now.saturating_add(validity)),
                active,
                last_used: now,
            },
        );
    }

    fn make_room_for(&mut self, peer: SocketAddr, now: Duration) {
        if self.peers.contains_key(&peer) || self.peers.len() < self.peer_limit {
            return;
        }
        let candidate = self
            .peers
            .iter()
            .min_by_key(|(_, state)| {
                let active = state
                    .active
                    .as_ref()
                    .is_some_and(|_| state.until.is_some_and(|until| now < until));
                (active, state.last_used)
            })
            .map(|(peer, _)| *peer);
        if let Some(candidate) = candidate {
            self.peers.remove(&candidate);
        }
    }

    pub(crate) fn admit(
        &mut self,
        peer: SocketAddr,
        category: RequestCategory,
        now: Duration,
    ) -> bool {
        let Some(state) = self.peers.get_mut(&peer) else {
            return true;
        };
        state.last_used = now;
        if state.until.is_none_or(|until| now >= until) {
            state.active = None;
            state.until = None;
            return true;
        }
        let Some(active) = state.active.as_mut() else {
            return true;
        };
        match active {
            Active::Rate(rate) => rate.admit(now, category),
            Active::Loss {
                percentage,
                ordinary,
                protected,
            } => {
                match category {
                    RequestCategory::Ordinary => *ordinary = ordinary.saturating_add(1),
                    RequestCategory::Protected => *protected = protected.saturating_add(1),
                }
                let total = f64::from(ordinary.saturating_add(*protected));
                let ordinary_share = (f64::from(*ordinary) / total) * 100.0;
                let protected_share = (f64::from(*protected) / total) * 100.0;
                let discard = match category {
                    RequestCategory::Ordinary if *percentage <= ordinary_share => {
                        *percentage / ordinary_share
                    }
                    RequestCategory::Ordinary => 1.0,
                    RequestCategory::Protected if *percentage <= ordinary_share => 0.0,
                    RequestCategory::Protected if protected_share > 0.0 => {
                        (*percentage - ordinary_share) / protected_share
                    }
                    RequestCategory::Protected => 0.0,
                };
                self.random.random::<f64>() >= discard.clamp(0.0, 1.0)
            }
        }
    }
}

/// Add the client capability parameters and remove server-only parameters from the top Via.
pub(crate) fn advertise(request: &mut Request) {
    rewrite_top_via(&mut request.headers, b";oc;oc-algo=\"loss,rate\"");
}

/// Add server feedback if the request offered the configured algorithm.
pub(crate) fn add_feedback(
    response: &mut Response,
    request: &Request,
    feedback: OverloadFeedback,
    validity: Duration,
    sequence: OverloadSequence,
) -> bool {
    let offered = request
        .headers
        .typed::<Via>()
        .and_then(Result::ok)
        .and_then(|via| via.overload().ok())
        .is_some_and(|parameters| {
            parameters.oc == Some(OcParameter::Support)
                && parameters.algorithms.iter().any(|algorithm| {
                    matches!(
                        (feedback, algorithm),
                        (OverloadFeedback::Loss(_), OverloadAlgorithm::Loss)
                            | (OverloadFeedback::Rate(_), OverloadAlgorithm::Rate)
                    )
                })
        });
    if !offered {
        return false;
    }
    let (value, algorithm) = match feedback {
        OverloadFeedback::Loss(value) => (u64::from(value), "loss"),
        OverloadFeedback::Rate(value) => (u64::from(value), "rate"),
    };
    let validity = u64::try_from(validity.as_millis()).unwrap_or(u64::MAX);
    let addition =
        format!(";oc={value};oc-algo=\"{algorithm}\";oc-validity={validity};oc-seq={sequence}");
    rewrite_top_via(&mut response.headers, addition.as_bytes())
}

fn report_from(response: &Response) -> Option<Report> {
    let parameters = response.headers.typed::<Via>()?.ok()?.overload().ok()?;
    let OcParameter::Value(value) = parameters.oc? else {
        return None;
    };
    let [algorithm] = parameters.algorithms.as_slice() else {
        return None;
    };
    Some(Report {
        algorithm: algorithm.clone(),
        value: u32::try_from(value).ok()?,
        validity: parameters.validity,
        sequence: parameters.sequence?,
    })
}

fn rewrite_top_via(headers: &mut Headers, addition: &[u8]) -> bool {
    let Some(header) = headers.get(&HeaderName::Via) else {
        return false;
    };
    let value = header.value().into_owned();
    let hop_end = first_hop_end(&value);
    let mut hop = value.get(..hop_end).unwrap_or(&value).to_vec();
    if Via::parse_one(&hop).is_err() {
        return false;
    }
    for name in [b"oc".as_slice(), b"oc-algo", b"oc-validity", b"oc-seq"] {
        while let Some((start, end)) = crate::nat::param_span(&hop, name) {
            hop.drain(start..end);
        }
    }
    hop.extend_from_slice(addition);
    let mut rebuilt = Vec::with_capacity(value.len().saturating_add(addition.len()));
    rebuilt.extend_from_slice(&hop);
    rebuilt.extend_from_slice(value.get(hop_end..).unwrap_or(&[]));
    let Ok(header) = Header::build(HeaderName::Via, Bytes::from(rebuilt)) else {
        return false;
    };
    if headers.remove_first(&HeaderName::Via).is_none() {
        return false;
    }
    headers.push_front(header);
    true
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use std::net::SocketAddr;
    use std::time::Duration;

    use bytes::Bytes;
    use sipx_sip::headers::{OcParameter, OverloadAlgorithm, OverloadSequence, Via};
    use sipx_sip::{Limits, Message, StatusCode, parse_datagram};

    use super::{Controller, OverloadFeedback, Report, RequestCategory, add_feedback, advertise};

    fn peer() -> SocketAddr {
        "192.0.2.10:5060".parse().expect("peer")
    }

    fn sequence(value: u64) -> OverloadSequence {
        OverloadSequence::from_integer(value).expect("small sequence")
    }

    fn report(algorithm: OverloadAlgorithm, value: u32, sequence_number: u64) -> Report {
        Report {
            algorithm,
            value,
            validity: Some(Duration::from_secs(10)),
            sequence: sequence(sequence_number),
        }
    }

    /// T-22's failing-first witness. A fixed seed makes the actual reduction reviewable instead of
    /// accepting whichever distribution an operating-system generator happened to produce.
    #[test]
    fn a_client_told_to_reduce_by_half_forwards_half_as_many_requests() {
        let mut controller = Controller::seeded(0x7339, 0, 0);
        controller.apply(
            peer(),
            &report(OverloadAlgorithm::Loss, 50, 1),
            Duration::ZERO,
        );

        let forwarded = (0..10_000)
            .filter(|_| {
                controller.admit(peer(), RequestCategory::Ordinary, Duration::from_millis(1))
            })
            .count();
        assert!(
            (4_900..=5_100).contains(&forwarded),
            "50% loss forwarded {forwarded}/10000 for the fixed seed"
        );
    }

    #[test]
    fn protected_requests_survive_while_ordinary_traffic_can_supply_the_reduction() {
        let mut controller = Controller::seeded(9, 0, 0);
        controller.apply(
            peer(),
            &report(OverloadAlgorithm::Loss, 50, 1),
            Duration::ZERO,
        );
        for _ in 0..80 {
            let _ = controller.admit(peer(), RequestCategory::Ordinary, Duration::ZERO);
        }
        for _ in 0..20 {
            assert!(controller.admit(peer(), RequestCategory::Protected, Duration::ZERO));
        }
    }

    #[test]
    fn stale_reports_and_expired_reports_do_not_control_the_client() {
        let mut controller = Controller::seeded(2, 0, 0);
        controller.apply(
            peer(),
            &report(OverloadAlgorithm::Loss, 100, 2),
            Duration::ZERO,
        );
        controller.apply(
            peer(),
            &report(OverloadAlgorithm::Loss, 0, 1),
            Duration::ZERO,
        );
        assert!(!controller.admit(peer(), RequestCategory::Ordinary, Duration::ZERO));

        controller.apply(
            peer(),
            &Report {
                algorithm: OverloadAlgorithm::Loss,
                value: 100,
                validity: Some(Duration::ZERO),
                sequence: sequence(3),
            },
            Duration::ZERO,
        );
        assert!(controller.admit(peer(), RequestCategory::Ordinary, Duration::ZERO));

        controller.apply(
            peer(),
            &report(OverloadAlgorithm::Loss, 100, 2),
            Duration::ZERO,
        );
        assert!(controller.admit(peer(), RequestCategory::Ordinary, Duration::ZERO));

        controller.apply(
            peer(),
            &Report {
                validity: Some(Duration::from_millis(10)),
                sequence: sequence(4),
                ..report(OverloadAlgorithm::Loss, 100, 4)
            },
            Duration::ZERO,
        );
        assert!(controller.admit(peer(), RequestCategory::Ordinary, Duration::from_millis(10)));
    }

    #[test]
    fn absent_validity_means_five_hundred_milliseconds_and_a_newer_report_restarts_control() {
        let mut controller = Controller::seeded(4, 0, 0);
        controller.apply(
            peer(),
            &Report {
                algorithm: OverloadAlgorithm::Loss,
                value: 100,
                validity: None,
                sequence: sequence(1),
            },
            Duration::ZERO,
        );
        assert!(!controller.admit(
            peer(),
            RequestCategory::Ordinary,
            Duration::from_millis(499)
        ));
        assert!(controller.admit(
            peer(),
            RequestCategory::Ordinary,
            Duration::from_millis(500)
        ));

        controller.apply(
            peer(),
            &report(OverloadAlgorithm::Loss, 100, 2),
            Duration::from_millis(501),
        );
        assert!(!controller.admit(
            peer(),
            RequestCategory::Ordinary,
            Duration::from_millis(501)
        ));
    }

    #[test]
    fn rate_control_paces_against_supplied_time() {
        let mut controller = Controller::seeded(3, 0, 0);
        controller.apply(
            peer(),
            &report(OverloadAlgorithm::Rate, 2, 1),
            Duration::ZERO,
        );
        assert!(controller.admit(peer(), RequestCategory::Ordinary, Duration::ZERO));
        assert!(!controller.admit(peer(), RequestCategory::Ordinary, Duration::ZERO));
        assert!(!controller.admit(
            peer(),
            RequestCategory::Ordinary,
            Duration::from_millis(499)
        ));
        assert!(controller.admit(
            peer(),
            RequestCategory::Ordinary,
            Duration::from_millis(500)
        ));
    }

    #[test]
    fn rate_burst_tolerance_is_an_input_not_a_hidden_constant() {
        let mut controller = Controller::seeded(5, 2, 2);
        controller.apply(
            peer(),
            &report(OverloadAlgorithm::Rate, 2, 1),
            Duration::ZERO,
        );
        assert!(controller.admit(peer(), RequestCategory::Ordinary, Duration::ZERO));
        assert!(controller.admit(peer(), RequestCategory::Ordinary, Duration::ZERO));
        assert!(controller.admit(peer(), RequestCategory::Ordinary, Duration::ZERO));
        assert!(!controller.admit(peer(), RequestCategory::Ordinary, Duration::ZERO));
    }

    #[test]
    fn rate_priority_uses_the_second_threshold_after_ordinary_traffic_is_blocked() {
        let mut controller = Controller::seeded(7, 0, 2);
        controller.apply(
            peer(),
            &report(OverloadAlgorithm::Rate, 2, 1),
            Duration::ZERO,
        );
        assert!(controller.admit(peer(), RequestCategory::Ordinary, Duration::ZERO));
        assert!(!controller.admit(peer(), RequestCategory::Ordinary, Duration::ZERO));
        assert!(controller.admit(peer(), RequestCategory::Protected, Duration::ZERO));
    }

    #[test]
    fn zero_rate_rejects_everything_but_zero_validity_disables_control() {
        let mut controller = Controller::seeded(6, 0, 0);
        controller.apply(
            peer(),
            &report(OverloadAlgorithm::Rate, 0, 1),
            Duration::ZERO,
        );
        assert!(!controller.admit(peer(), RequestCategory::Ordinary, Duration::ZERO));
        controller.apply(
            peer(),
            &Report {
                algorithm: OverloadAlgorithm::Rate,
                value: 0,
                validity: Some(Duration::ZERO),
                sequence: sequence(2),
            },
            Duration::ZERO,
        );
        assert!(controller.admit(peer(), RequestCategory::Ordinary, Duration::ZERO));
    }

    #[test]
    fn feedback_from_many_peers_never_exceeds_the_configured_state_bound() {
        let mut controller = Controller::seeded_with_limit(8, 0, 0, 4);

        for port in 5000..5100 {
            let peer = SocketAddr::from(([192, 0, 2, 10], port));
            controller.apply(
                peer,
                &report(OverloadAlgorithm::Loss, 100, u64::from(port)),
                Duration::from_millis(u64::from(port)),
            );
            assert!(
                controller.peers.len() <= 4,
                "peer state exceeded its configured bound"
            );
        }

        assert_eq!(controller.peers.len(), 4);
    }

    #[test]
    fn client_and_server_generate_only_the_parameters_their_role_owns() {
        let bytes = Bytes::from_static(
            b"OPTIONS sip:a@example SIP/2.0\r\n\
              Via: SIP/2.0/UDP client.example;branch=z9hG4bKx;oc=9;oc-algo=loss;\
              oc-validity=1;oc-seq=1.0\r\n\
              To: <sip:a@example>\r\n\
              From: <sip:b@example>;tag=one\r\n\
              Call-ID: roles@sipx\r\n\
              CSeq: 1 OPTIONS\r\n\
              Max-Forwards: 70\r\n\
              Content-Length: 0\r\n\r\n",
        );
        let Message::Request(mut request) =
            parse_datagram(bytes, &Limits::datagram()).expect("request parses")
        else {
            panic!("request expected");
        };
        advertise(&mut request);
        let offer = request
            .headers
            .typed::<Via>()
            .expect("Via")
            .expect("Via parses")
            .overload()
            .expect("offer parses");
        assert_eq!(offer.oc, Some(OcParameter::Support));
        assert_eq!(
            offer.algorithms,
            vec![OverloadAlgorithm::Loss, OverloadAlgorithm::Rate]
        );
        assert_eq!(offer.validity, None);
        assert_eq!(offer.sequence, None);

        let status = StatusCode::new(503).expect("status");
        let mut response =
            sipx_sip::ResponseBuilder::to_request(&request, status, "Service Unavailable")
                .expect("response")
                .build();
        assert!(add_feedback(
            &mut response,
            &request,
            OverloadFeedback::Rate(150),
            Duration::from_secs(1),
            sequence(2),
        ));
        let report = response
            .headers
            .typed::<Via>()
            .expect("Via")
            .expect("Via parses")
            .overload()
            .expect("report parses");
        assert_eq!(report.oc, Some(OcParameter::Value(150)));
        assert_eq!(report.algorithms, vec![OverloadAlgorithm::Rate]);
        assert_eq!(report.validity, Some(Duration::from_secs(1)));
        assert_eq!(report.sequence, Some(sequence(2)));
        assert!(
            response
                .headers
                .value(&sipx_sip::HeaderName::Via)
                .is_some_and(|value| {
                    value
                        .windows(b"oc-algo=\"rate\"".len())
                        .any(|part| part == b"oc-algo=\"rate\"")
                }),
            "the server algorithm token is a quoted algo-list on the wire"
        );
    }
}
