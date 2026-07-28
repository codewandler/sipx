//! RFC 3263: turning a SIP URI into an ordered list of places to try.
//!
//! The RFC's procedure is short to state and easy to get subtly wrong. What matters:
//!
//! - An IP literal or an explicit port means no lookup at all. The URI has already answered.
//! - NAPTR chooses the *transport*; SRV chooses the *host and port*; A/AAAA chooses the
//!   address. Skipping a stage changes which deployments are reachable.
//! - `sips:` restricts the candidates to TLS. Falling back to UDP because TLS was unavailable
//!   would silently downgrade a request the user asked to be secure.
//! - The result is a *list*. One candidate failing is normal, and the request has not failed
//!   until the list is exhausted.
//!
//! DNS itself is behind a trait: tests use a fixture and never touch a resolver.

use std::net::{IpAddr, SocketAddr};

use sipx_sip::{Host, Uri};

use crate::target::{Target, TransportKind};

/// A NAPTR record, reduced to what RFC 3263 uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Naptr {
    /// Lower is preferred.
    pub order: u16,
    /// Lower is preferred, within an order.
    pub preference: u16,
    /// `SIP+D2U`, `SIPS+D2T` and friends.
    pub service: String,
    /// The SRV name to look up next.
    pub replacement: String,
}

/// An SRV record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Srv {
    /// Lower is preferred.
    pub priority: u16,
    /// Relative share within a priority (RFC 2782).
    pub weight: u16,
    /// The port to use.
    pub port: u16,
    /// The host to resolve.
    pub target: String,
}

/// What a resolver must be able to answer.
///
/// A trait rather than a concrete DNS client so that the selection logic — which is where the
/// bugs live — is testable without a network.
pub trait Resolver: Send + Sync {
    /// NAPTR records for a domain.
    fn naptr(&self, domain: &str) -> Vec<Naptr>;
    /// SRV records for a name.
    fn srv(&self, name: &str) -> Vec<Srv>;
    /// Addresses for a host.
    fn addresses(&self, host: &str) -> Vec<IpAddr>;
}

/// A source of randomness for RFC 2782 weighted selection.
///
/// Injectable so the distribution is testable with a fixed seed; a test that cannot pin the
/// randomness can only assert that selection did *something*.
pub trait Rng: Send + Sync {
    /// A value in `0..=max`.
    fn below(&mut self, max: u32) -> u32;
}

/// The thread RNG.
#[derive(Debug, Default)]
pub struct OsRng;

impl Rng for OsRng {
    fn below(&mut self, max: u32) -> u32 {
        if max == 0 {
            return 0;
        }
        rand::Rng::random_range(&mut rand::rng(), 0..=max)
    }
}

/// A deterministic RNG for tests: a linear congruential generator with a fixed seed.
#[derive(Debug)]
pub struct SeededRng(u64);

impl SeededRng {
    /// A generator with the given seed.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
}

impl Rng for SeededRng {
    fn below(&mut self, max: u32) -> u32 {
        // Numerical Recipes' constants. Adequate for choosing among SRV records; this is not
        // used for anything security-relevant, which is why `new_branch` does not use it.
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        if max == 0 {
            return 0;
        }
        u32::try_from((self.0 >> 33) % u64::from(max) + 1).unwrap_or(0)
    }
}

/// Which transports a scheme permits.
fn permitted(uri: &Uri) -> Vec<TransportKind> {
    if uri.scheme().is_secure() {
        // A `sips:` URI is a request for TLS. Falling back to UDP because TLS was unavailable
        // would silently downgrade exactly the thing the scheme asked for.
        vec![TransportKind::Tls, TransportKind::Wss]
    } else {
        vec![
            TransportKind::Udp,
            TransportKind::Tcp,
            TransportKind::Tls,
            TransportKind::Ws,
            TransportKind::Wss,
        ]
    }
}

/// The transport a URI names explicitly, if any.
fn explicit_transport(uri: &Uri) -> Option<TransportKind> {
    uri.transport().and_then(TransportKind::parse)
}

/// Map a NAPTR service field to a transport (RFC 3263 §4.1).
fn service_transport(service: &str) -> Option<TransportKind> {
    match service.to_ascii_uppercase().as_str() {
        "SIP+D2U" => Some(TransportKind::Udp),
        "SIP+D2T" => Some(TransportKind::Tcp),
        "SIPS+D2T" => Some(TransportKind::Tls),
        "SIP+D2W" => Some(TransportKind::Ws),
        "SIPS+D2W" => Some(TransportKind::Wss),
        _ => None,
    }
}

/// The conventional SRV prefix for a transport.
fn srv_prefix(transport: TransportKind) -> &'static str {
    match transport {
        TransportKind::Udp => "_sip._udp.",
        TransportKind::Tcp => "_sip._tcp.",
        TransportKind::Tls => "_sips._tcp.",
        TransportKind::Ws => "_sip._ws.",
        TransportKind::Wss => "_sips._wss.",
    }
}

/// Resolve a URI to an ordered list of candidates.
///
/// The list is tried in order; a transport failure moves to the next. The request has not
/// failed until every candidate has.
pub fn resolve<R: Resolver + ?Sized, G: Rng + ?Sized>(
    uri: &Uri,
    resolver: &R,
    rng: &mut G,
) -> Vec<Target> {
    let allowed = permitted(uri);
    let default_transport = explicit_transport(uri).unwrap_or(if uri.scheme().is_secure() {
        TransportKind::Tls
    } else {
        TransportKind::Udp
    });

    // §4.2: an IP literal, or an explicit port, ends the procedure. The URI has answered.
    if let Some(Host::Ip(ip)) = uri.host() {
        let port = uri
            .port()
            .unwrap_or_else(|| default_transport.default_port());
        return vec![Target::new(SocketAddr::new(*ip, port), default_transport)];
    }

    let Some(Host::Name(name)) = uri.host() else {
        return Vec::new();
    };
    let domain = String::from_utf8_lossy(name.as_bytes()).into_owned();

    if let Some(port) = uri.port() {
        // A port was given, so no SRV lookup — but the name still has to become an address.
        return resolver
            .addresses(&domain)
            .into_iter()
            .map(|ip| Target::new(SocketAddr::new(ip, port), default_transport))
            .collect();
    }

    // An explicit `transport=` parameter skips NAPTR: the caller has already chosen.
    let transports: Vec<(TransportKind, String)> = match explicit_transport(uri) {
        Some(transport) => vec![(transport, format!("{}{domain}", srv_prefix(transport)))],
        None => naptr_transports(&domain, resolver, &allowed),
    };

    let mut targets = Vec::new();
    for (transport, srv_name) in transports {
        if !allowed.contains(&transport) {
            continue;
        }
        let records = resolver.srv(&srv_name);
        if records.is_empty() {
            continue;
        }
        for srv in order_srv(records, rng) {
            for ip in resolver.addresses(&srv.target) {
                targets.push(Target::new(SocketAddr::new(ip, srv.port), transport));
            }
        }
    }

    if !targets.is_empty() {
        return targets;
    }

    // §4.2 last resort: no NAPTR, no SRV — resolve the name and use the default port.
    resolver
        .addresses(&domain)
        .into_iter()
        .map(|ip| {
            Target::new(
                SocketAddr::new(ip, default_transport.default_port()),
                default_transport,
            )
        })
        .collect()
}

/// NAPTR lookup, reduced to an ordered list of (transport, SRV name).
///
/// When there are no NAPTR records the RFC says to try the SRV names directly, in an order of
/// the implementation's choosing among the transports it supports.
fn naptr_transports<R: Resolver + ?Sized>(
    domain: &str,
    resolver: &R,
    allowed: &[TransportKind],
) -> Vec<(TransportKind, String)> {
    let mut records = resolver.naptr(domain);
    if records.is_empty() {
        return allowed
            .iter()
            .filter(|t| !matches!(t, TransportKind::Ws | TransportKind::Wss))
            .map(|&t| (t, format!("{}{domain}", srv_prefix(t))))
            .collect();
    }

    // Order first, then preference — both ascending, both "lower is better".
    records.sort_by_key(|r| (r.order, r.preference));
    records
        .into_iter()
        .filter_map(|record| {
            let transport = service_transport(&record.service)?;
            Some((transport, record.replacement))
        })
        .collect()
}

/// Order SRV records: priority ascending, and within a priority the RFC 2782 weighted shuffle.
fn order_srv<G: Rng + ?Sized>(mut records: Vec<Srv>, rng: &mut G) -> Vec<Srv> {
    records.sort_by_key(|r| r.priority);

    let mut ordered = Vec::with_capacity(records.len());
    let mut rest = records;
    while !rest.is_empty() {
        let priority = rest.first().map_or(0, |r| r.priority);
        let mut group: Vec<Srv> = Vec::new();
        let mut remainder: Vec<Srv> = Vec::new();
        for record in rest {
            if record.priority == priority {
                group.push(record);
            } else {
                remainder.push(record);
            }
        }
        ordered.extend(weighted_shuffle(group, rng));
        rest = remainder;
    }
    ordered
}

/// RFC 2782's selection: pick with probability proportional to weight, repeatedly.
///
/// The RFC's own wording — running sum, pick a random number in `0..=total`, take the first
/// entry whose running sum is at least that number. A weight of 0 is legal and means "only if
/// nothing else is available", which falls out of the arithmetic rather than needing a case.
fn weighted_shuffle<G: Rng + ?Sized>(mut group: Vec<Srv>, rng: &mut G) -> Vec<Srv> {
    let mut ordered = Vec::with_capacity(group.len());
    while !group.is_empty() {
        let total: u32 = group.iter().map(|r| u32::from(r.weight)).sum();
        let pick = rng.below(total);

        let mut running = 0u32;
        let mut chosen = group.len().saturating_sub(1);
        for (index, record) in group.iter().enumerate() {
            running += u32::from(record.weight);
            if running >= pick {
                chosen = index;
                break;
            }
        }
        if chosen < group.len() {
            ordered.push(group.remove(chosen));
        }
    }
    ordered
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
    use std::collections::HashMap;

    #[derive(Debug, Default)]
    struct Fixture {
        naptr: HashMap<String, Vec<Naptr>>,
        srv: HashMap<String, Vec<Srv>>,
        addresses: HashMap<String, Vec<IpAddr>>,
    }

    impl Fixture {
        fn with_address(mut self, host: &str, addr: &str) -> Self {
            self.addresses
                .entry(host.to_owned())
                .or_default()
                .push(addr.parse().expect("a valid address"));
            self
        }

        fn with_srv(mut self, name: &str, records: Vec<Srv>) -> Self {
            self.srv.insert(name.to_owned(), records);
            self
        }

        fn with_naptr(mut self, domain: &str, records: Vec<Naptr>) -> Self {
            self.naptr.insert(domain.to_owned(), records);
            self
        }
    }

    impl Resolver for Fixture {
        fn naptr(&self, domain: &str) -> Vec<Naptr> {
            self.naptr.get(domain).cloned().unwrap_or_default()
        }
        fn srv(&self, name: &str) -> Vec<Srv> {
            self.srv.get(name).cloned().unwrap_or_default()
        }
        fn addresses(&self, host: &str) -> Vec<IpAddr> {
            self.addresses.get(host).cloned().unwrap_or_default()
        }
    }

    fn uri(text: &str) -> Uri {
        Uri::parse(bytes::Bytes::from(text.to_owned())).expect("a valid URI")
    }

    fn srv(priority: u16, weight: u16, port: u16, target: &str) -> Srv {
        Srv {
            priority,
            weight,
            port,
            target: target.to_owned(),
        }
    }

    /// §4.2: an IP literal has already answered the question.
    #[test]
    fn an_ip_literal_short_circuits_resolution() {
        let targets = resolve(
            &uri("sip:192.0.2.10:5080"),
            &Fixture::default(),
            &mut SeededRng::new(1),
        );
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].addr.to_string(), "192.0.2.10:5080");
        assert_eq!(targets[0].transport, TransportKind::Udp);
    }

    #[test]
    fn an_ip_literal_without_a_port_uses_the_transport_default() {
        let targets = resolve(
            &uri("sips:192.0.2.10"),
            &Fixture::default(),
            &mut SeededRng::new(1),
        );
        assert_eq!(targets[0].addr.port(), 5061);
        assert_eq!(targets[0].transport, TransportKind::Tls);
    }

    /// An explicit port means no SRV lookup — but the name still has to be resolved.
    #[test]
    fn an_explicit_port_skips_srv_but_not_the_address_lookup() {
        let fixture = Fixture::default()
            .with_address("example.com", "192.0.2.20")
            .with_srv("_sip._udp.example.com", vec![srv(1, 1, 9999, "wrong.com")]);
        let targets = resolve(
            &uri("sip:example.com:5080"),
            &fixture,
            &mut SeededRng::new(1),
        );
        assert_eq!(targets.len(), 1);
        assert_eq!(
            targets[0].addr.to_string(),
            "192.0.2.20:5080",
            "the SRV port must not override an explicit one"
        );
    }

    #[test]
    fn naptr_chooses_the_transport_and_srv_the_port() {
        let fixture = Fixture::default()
            .with_naptr(
                "example.com",
                vec![
                    Naptr {
                        order: 20,
                        preference: 10,
                        service: "SIP+D2U".to_owned(),
                        replacement: "_sip._udp.example.com".to_owned(),
                    },
                    Naptr {
                        order: 10,
                        preference: 10,
                        service: "SIP+D2T".to_owned(),
                        replacement: "_sip._tcp.example.com".to_owned(),
                    },
                ],
            )
            .with_srv(
                "_sip._tcp.example.com",
                vec![srv(1, 0, 5060, "tcp.example.com")],
            )
            .with_srv(
                "_sip._udp.example.com",
                vec![srv(1, 0, 5060, "udp.example.com")],
            )
            .with_address("tcp.example.com", "192.0.2.30")
            .with_address("udp.example.com", "192.0.2.31");

        let targets = resolve(&uri("sip:example.com"), &fixture, &mut SeededRng::new(1));
        assert_eq!(
            targets[0].transport,
            TransportKind::Tcp,
            "order 10 is preferred over order 20"
        );
        assert_eq!(targets[0].addr.to_string(), "192.0.2.30:5060");
        assert_eq!(targets[1].transport, TransportKind::Udp);
    }

    /// A `sips:` URI is a request for TLS. Falling back to UDP because TLS was unavailable
    /// would silently downgrade exactly what the scheme asked for.
    #[test]
    fn sips_never_yields_a_cleartext_candidate() {
        let fixture = Fixture::default()
            .with_naptr(
                "secure.example",
                vec![
                    Naptr {
                        order: 10,
                        preference: 10,
                        service: "SIP+D2U".to_owned(),
                        replacement: "_sip._udp.secure.example".to_owned(),
                    },
                    Naptr {
                        order: 20,
                        preference: 10,
                        service: "SIPS+D2T".to_owned(),
                        replacement: "_sips._tcp.secure.example".to_owned(),
                    },
                ],
            )
            .with_srv(
                "_sip._udp.secure.example",
                vec![srv(1, 0, 5060, "plain.secure.example")],
            )
            .with_srv(
                "_sips._tcp.secure.example",
                vec![srv(1, 0, 5061, "tls.secure.example")],
            )
            .with_address("plain.secure.example", "192.0.2.40")
            .with_address("tls.secure.example", "192.0.2.41");

        let targets = resolve(
            &uri("sips:secure.example"),
            &fixture,
            &mut SeededRng::new(1),
        );
        assert!(!targets.is_empty(), "TLS is available and must be found");
        for target in &targets {
            assert!(
                matches!(target.transport, TransportKind::Tls | TransportKind::Wss),
                "sips must not yield {:?}",
                target.transport
            );
        }
    }

    /// And when TLS is *not* available, the answer is no candidates — not a downgrade.
    #[test]
    fn sips_with_no_tls_available_yields_nothing_rather_than_downgrading() {
        let fixture = Fixture::default()
            .with_naptr(
                "plain.example",
                vec![Naptr {
                    order: 10,
                    preference: 10,
                    service: "SIP+D2U".to_owned(),
                    replacement: "_sip._udp.plain.example".to_owned(),
                }],
            )
            .with_srv(
                "_sip._udp.plain.example",
                vec![srv(1, 0, 5060, "host.plain.example")],
            );
        let targets = resolve(&uri("sips:plain.example"), &fixture, &mut SeededRng::new(1));
        assert!(targets.is_empty());
    }

    #[test]
    fn an_explicit_transport_parameter_skips_naptr() {
        let fixture = Fixture::default()
            .with_naptr(
                "example.com",
                vec![Naptr {
                    order: 10,
                    preference: 10,
                    service: "SIP+D2U".to_owned(),
                    replacement: "_sip._udp.example.com".to_owned(),
                }],
            )
            .with_srv(
                "_sip._tcp.example.com",
                vec![srv(1, 0, 5060, "t.example.com")],
            )
            .with_address("t.example.com", "192.0.2.50");

        let targets = resolve(
            &uri("sip:example.com;transport=tcp"),
            &fixture,
            &mut SeededRng::new(1),
        );
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].transport, TransportKind::Tcp);
    }

    #[test]
    fn priority_is_absolute_and_weight_only_orders_within_it() {
        let fixture = Fixture::default()
            .with_srv(
                "_sip._udp.example.com",
                vec![
                    srv(20, 100, 5060, "low.example.com"),
                    srv(10, 1, 5060, "high.example.com"),
                ],
            )
            .with_address("low.example.com", "192.0.2.60")
            .with_address("high.example.com", "192.0.2.61");

        for seed in 0..20 {
            let targets = resolve(&uri("sip:example.com"), &fixture, &mut SeededRng::new(seed));
            assert_eq!(
                targets[0].addr.ip().to_string(),
                "192.0.2.61",
                "priority 10 always precedes priority 20, whatever the weights"
            );
        }
    }

    /// RFC 2782 weighted selection: over many draws, the share of first-picks should track the
    /// weights. With 10 and 90 the split should be near one in ten.
    #[test]
    fn srv_weighted_selection_matches_rfc2782_distribution() {
        let records = vec![
            srv(1, 10, 5060, "light.example"),
            srv(1, 90, 5060, "heavy.example"),
        ];

        let mut light_first: i32 = 0;
        let draws: i32 = 4000;
        for seed in 0..u64::try_from(draws).unwrap_or(0) {
            let mut rng = SeededRng::new(seed);
            let ordered = weighted_shuffle(records.clone(), &mut rng);
            if ordered.first().map(|r| r.target.as_str()) == Some("light.example") {
                light_first += 1;
            }
        }

        let share = f64::from(light_first) / f64::from(draws);
        assert!(
            (0.05..0.16).contains(&share),
            "a weight of 10 against 90 should win about a tenth of the time, got {share}"
        );
    }

    /// A weight of 0 is legal and means "only if nothing else is available".
    #[test]
    fn a_zero_weight_record_is_still_reachable() {
        let records = vec![
            srv(1, 0, 5060, "spare.example"),
            srv(1, 100, 5060, "main.example"),
        ];
        let ordered = weighted_shuffle(records, &mut SeededRng::new(7));
        assert_eq!(ordered.len(), 2, "every record appears exactly once");
        assert!(ordered.iter().any(|r| r.target == "spare.example"));
    }

    /// Every record must survive the shuffle. Losing one silently removes a server from
    /// rotation, which is the kind of bug that shows up as capacity that is never used.
    #[test]
    fn the_shuffle_is_a_permutation() {
        let records = vec![
            srv(1, 1, 5060, "a"),
            srv(1, 2, 5060, "b"),
            srv(1, 3, 5060, "c"),
            srv(1, 0, 5060, "d"),
        ];
        for seed in 0..50 {
            let ordered = weighted_shuffle(records.clone(), &mut SeededRng::new(seed));
            let mut names: Vec<&str> = ordered.iter().map(|r| r.target.as_str()).collect();
            names.sort_unstable();
            assert_eq!(names, vec!["a", "b", "c", "d"]);
        }
    }

    /// §4.2's last resort: no NAPTR, no SRV, just an A record and the default port.
    #[test]
    fn a_bare_a_record_is_the_last_resort() {
        let fixture = Fixture::default().with_address("simple.example", "192.0.2.70");
        let targets = resolve(&uri("sip:simple.example"), &fixture, &mut SeededRng::new(1));
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].addr.to_string(), "192.0.2.70:5060");
        assert_eq!(targets[0].transport, TransportKind::Udp);
    }

    #[test]
    fn a_name_that_resolves_to_nothing_yields_no_candidates() {
        let targets = resolve(
            &uri("sip:nowhere.example"),
            &Fixture::default(),
            &mut SeededRng::new(1),
        );
        assert!(targets.is_empty());
    }

    /// Multiple addresses for one SRV target are all candidates — falling through them is the
    /// point of returning a list.
    #[test]
    fn every_address_of_a_target_becomes_a_candidate() {
        let fixture = Fixture::default()
            .with_srv(
                "_sip._udp.example.com",
                vec![srv(1, 0, 5060, "multi.example.com")],
            )
            .with_address("multi.example.com", "192.0.2.80")
            .with_address("multi.example.com", "192.0.2.81");
        let targets = resolve(&uri("sip:example.com"), &fixture, &mut SeededRng::new(1));
        assert_eq!(targets.len(), 2);
    }
}
