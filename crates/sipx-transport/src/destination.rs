//! Bounded outbound SIP destination resolution, for anything that places a request.
//!
//! [`crate::dns`] is the client and [`crate::resolve`] is the selection; this is the thing a
//! caller actually holds. It carries one nameserver client, one cache and one finite policy
//! across every destination a process resolves, and turns a URI — with or without an explicit
//! next hop — into the ordered candidate list RFC 3263 defines.
//!
//! Three properties are the reason this is a type rather than a function.
//!
//! **Every resolver states its budget.** [`Resolver::within`] is the only way to build one from
//! the host's configuration, and it takes the caller's deadline. A caller holding a two-second
//! attempt deadline must not start an eight-second resolution beside it: the answer would arrive
//! after it stopped being wanted, and both bounds would be honest about different things.
//!
//! **The name outlives the address.** Every secure candidate is verified against the host the
//! caller named, even when an explicit next hop sends the request somewhere else entirely. An
//! identity derived from the resolved address would let whoever can influence DNS choose which
//! certificate is acceptable.
//!
//! **A failure says whose it is.** [`Kind`] separates a zone that has no answer from a resolver
//! that never replied from input that was refused before any lookup — three conditions with
//! three different owners and three different fixes.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use sipx_sip::{Host, Uri};

use crate::dns::{DnsResolver, ResolutionError, ResolutionPolicy, resolve_uri_bounded};
use crate::resolve::OsRng;
use crate::{Target, TransportKind};

/// The finite serial connection-attempt budget over one resolved candidate list.
///
/// Resolution is bounded, and trying what it returned has to be bounded too: a domain that
/// answers with a hundred SRV targets would otherwise turn one call into a hundred serial
/// connection attempts, each with its own timeout, long after the caller gave up.
pub const MAX_ATTEMPTS: usize = 16;

/// The nameserver to ask, when it should not be the one the host is configured with.
///
/// A caller that names a destination and cannot say *which* resolver it asked cannot tell a zone
/// that is wrong from a resolver that is unreachable — and those have different owners and
/// different fixes. It is read from the environment rather than passed as an argument because it
/// describes the machine the process runs on, not the request being placed; a caller that knows
/// its own nameserver hands the client to [`Resolver::over`] instead.
const NAMESERVER: &str = "SIPX_NAMESERVER";

/// Port 53, when `SIPX_NAMESERVER` names an address and not a port (RFC 1035 §4.2).
const DNS_PORT: u16 = 53;

/// A reusable resolver. One of these gets one nameserver client, one cache and one finite policy.
#[derive(Debug)]
pub struct Resolver {
    dns: Result<Arc<DnsResolver>, String>,
    policy: ResolutionPolicy,
}

impl Resolver {
    /// Read the process' configured nameservers, or the one `SIPX_NAMESERVER` names.
    fn system() -> Self {
        let policy = ResolutionPolicy::default();
        Self {
            dns: configured(policy)
                .map(Arc::new)
                .map_err(|source| source.to_string()),
            policy,
        }
    }

    /// A resolver reading the host's DNS configuration, whose own deadlines fit inside `budget`.
    ///
    /// Resolution is already bounded: one deadline per question and one over the whole of it,
    /// two and eight seconds by default. A caller that also has a deadline over the *attempt*
    /// must not start a second clock beside those — a resolver still entitled to eight seconds
    /// inside a two-second attempt answers after the answer stopped being wanted. So the budget
    /// is pushed down into the same policy rather than raced against it: neither resolution
    /// deadline may exceed what the attempt has left.
    ///
    /// `None` is a caller that states no deadline at all, and the two default bounds are then the
    /// whole of it. It is spelled here rather than at the call site so that a caller which gains a
    /// deadline later cannot keep an unbounded resolver by forgetting to say so.
    #[must_use]
    pub fn within(budget: Option<Duration>) -> Self {
        Self::system().narrowed(budget)
    }

    /// A resolver over a nameserver client the caller built, under the same bounds.
    ///
    /// [`Resolver::within`] reads the host's configuration, which is what an application running
    /// on that host wants. A caller that already knows which nameserver to ask — a service whose
    /// resolver is not the machine's, or a test with a fixture zone — hands the client in.
    ///
    /// The client's own per-question wait is separate from, and no substitute for, the bounds
    /// here: the deadline that fires is whichever is shorter, and only the ones in `budget` are
    /// reported as sipx's own.
    #[must_use]
    pub fn over(dns: Arc<DnsResolver>, budget: Option<Duration>) -> Self {
        Self {
            dns: Ok(dns),
            policy: ResolutionPolicy::default(),
        }
        .narrowed(budget)
    }

    /// The same nameserver client, and the same cache, under one attempt's budget.
    ///
    /// A process placing many requests, each with its own deadline, would throw away what the
    /// previous lookups established — including the negative answers — if it built a client per
    /// request. This shares both and narrows only the policy.
    #[must_use]
    pub fn narrowed(&self, budget: Option<Duration>) -> Self {
        let mut resolver = Self {
            dns: self.dns.clone(),
            policy: self.policy,
        };
        let Some(budget) = budget else {
            return resolver;
        };
        // Never zero: `resolve_uri_bounded` refuses a deadline that cannot bound work, and a
        // caller with nothing left to spend has already given up before reaching here.
        let ceiling = budget.max(Duration::from_millis(1));
        resolver.policy.resolution_timeout = resolver.policy.resolution_timeout.min(ceiling);
        resolver.policy.lookup_timeout = resolver
            .policy
            .lookup_timeout
            .min(resolver.policy.resolution_timeout);
        resolver
    }

    /// The bounds this resolver actually resolves under, after every narrowing.
    #[must_use]
    pub fn policy(&self) -> ResolutionPolicy {
        self.policy
    }

    /// Resolve a URI, or an explicit next hop, into an ordered candidate list (RFC 3263).
    ///
    /// `next_hop` is a `host` or `host:port` the request must travel through — an outbound proxy,
    /// or the outermost `Route` of a preloaded route set. It replaces the host that is *resolved*
    /// and changes neither the request URI nor the identity below.
    ///
    /// `requested_transport` is explicit caller policy. `None` accepts whatever the URI scheme
    /// and the zone select, which is what RFC 3263 is for; naming one restricts discovery to that
    /// transport rather than filtering the results of a wider search.
    ///
    /// Every secure candidate carries the host the *URI* named as its TLS or WSS verification
    /// identity, including when `next_hop` sends the request elsewhere, and a `sips:` URI never
    /// yields a cleartext candidate. A caller that has separately validated some other name — an
    /// operator-configured server name, say — applies it with [`Target::verifying`] afterwards;
    /// nothing here will ever derive an identity from a resolved address.
    ///
    /// The list is in the order to try, and [`MAX_ATTEMPTS`] is how far down it to go.
    pub async fn resolve(
        &self,
        uri: &Uri,
        next_hop: Option<&str>,
        requested_transport: Option<TransportKind>,
    ) -> Result<Vec<Target>, Error> {
        let route = match next_hop {
            Some(raw) => route_uri(uri, raw)?,
            None => uri.clone(),
        };
        let identity = uri.host().map(ToString::to_string).unwrap_or_default();
        let mut rng = OsRng;
        let candidates = if matches!(route.host(), Some(Host::Ip(_))) {
            crate::resolve_bounded(
                &route,
                &NoDns,
                &mut rng,
                requested_transport,
                self.policy.limits,
            )
            .map_err(|error| Error::Resolution(ResolutionError::Selection(error)))?
        } else {
            let dns = self.dns.as_ref().map_err(|message| Error::Setup {
                message: message.clone(),
            })?;
            resolve_uri_bounded(&route, dns, &mut rng, requested_transport, self.policy)
                .await
                .map_err(Error::Resolution)?
        };

        Ok(candidates
            .into_iter()
            .map(|target| {
                if target.transport.is_secure() {
                    target.verifying(&identity)
                } else {
                    target
                }
            })
            .collect())
    }
}

/// Build the DNS client an invocation shares.
fn configured(policy: ResolutionPolicy) -> std::io::Result<DnsResolver> {
    let Some(value) = std::env::var_os(NAMESERVER) else {
        return DnsResolver::from_system();
    };
    let value = value.to_string_lossy().into_owned();
    let address = nameserver(&value).ok_or_else(|| {
        std::io::Error::other(format!(
            "{NAMESERVER} must be an IP address, optionally with a port, not {value:?}"
        ))
    })?;
    // The client's own per-question wait is the outer resolution bound rather than the
    // per-question one, so the deadline that fires — and is therefore the one reported — is always
    // sipx's own. `Resolver::narrowed` only ever lowers those, so this stays true under a caller's
    // deadline too.
    DnsResolver::for_nameserver(address, policy.resolution_timeout)
}

/// `address` or `address:port`. A name here would need a resolver to read, which is the thing
/// being configured.
fn nameserver(value: &str) -> Option<SocketAddr> {
    value.parse::<SocketAddr>().ok().or_else(|| {
        value
            .parse::<IpAddr>()
            .ok()
            .map(|address| SocketAddr::new(address, DNS_PORT))
    })
}

/// The resolver a literal address gets: an address needs no lookup, and asking anyway would make
/// a URI that cannot fail depend on a nameserver being reachable.
struct NoDns;

impl crate::Resolver for NoDns {
    fn naptr(&self, _domain: &str) -> Vec<crate::Naptr> {
        Vec::new()
    }

    fn srv(&self, _name: &str) -> Vec<crate::Srv> {
        Vec::new()
    }

    fn addresses(&self, _host: &str) -> Vec<IpAddr> {
        Vec::new()
    }
}

/// Why no candidate list was produced.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The caller's URI, next hop or transport policy was refused before any lookup.
    #[error("{message}")]
    Input {
        /// What was wrong with it.
        message: String,
    },
    /// No nameserver client could be built at all.
    #[error("DNS resolver setup failed: {message}")]
    Setup {
        /// Why not.
        message: String,
    },
    /// Resolution itself did not produce candidates.
    #[error("target resolution failed: {0}")]
    Resolution(ResolutionError),
}

impl Error {
    /// Which of the four conditions this is.
    #[must_use]
    pub fn kind(&self) -> Kind {
        match self {
            Self::Input { .. } => Kind::Input,
            Self::Setup { .. } => Kind::Setup,
            Self::Resolution(error) => match error {
                ResolutionError::InvalidTimeout { .. }
                | ResolutionError::Selection(
                    crate::ResolutionError::InvalidLimit { .. }
                    | crate::ResolutionError::InvalidTransport
                    | crate::ResolutionError::ConflictingTransport { .. }
                    | crate::ResolutionError::SecureTransportRequired { .. },
                ) => Kind::Input,
                ResolutionError::LookupTimeout { .. } | ResolutionError::ResolutionTimeout => {
                    Kind::Timeout
                }
                _ => Kind::Resolution,
            },
        }
    }
}

/// What kind of failure a resolution error is, for a caller that classifies rather than matches.
///
/// The distinction that costs the most to get wrong is the middle pair. A zone with no answer is
/// final and the next candidate — or a different destination — is the fix; a deadline says
/// nothing about the name at all and retrying is right. A caller that folds them together either
/// gives up on a name that was fine or retries one that never existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Kind {
    /// The caller asked for something impossible, and no lookup was made.
    Input,
    /// No nameserver client could be built, so nothing was asked.
    Setup,
    /// The questions were asked and produced no usable answer.
    Resolution,
    /// A deadline fired first. Not a statement about the name.
    Timeout,
}

/// Construct the URI to resolve for an explicit next hop, retaining the request URI's scheme,
/// port and transport policy.
fn route_uri(uri: &Uri, raw: &str) -> Result<Uri, Error> {
    let (host, next_hop_port) = Host::parse_hostport(&Bytes::copy_from_slice(raw.as_bytes()))
        .map_err(|_| Error::Input {
            message: format!("invalid next-hop host or host:port: {raw}"),
        })?;
    let port = next_hop_port.or_else(|| uri.port());
    let scheme = String::from_utf8_lossy(uri.scheme().as_bytes());
    let mut rendered = format!("{scheme}:{host}");
    if let Some(port) = port {
        rendered.push(':');
        rendered.push_str(&port.to_string());
    }
    if let Some(transport) = uri.transport() {
        rendered.push_str(";transport=");
        rendered.push_str(&String::from_utf8_lossy(transport));
    }
    Uri::parse(Bytes::from(rendered)).map_err(|_| Error::Input {
        message: format!("invalid next-hop host or host:port: {raw}"),
    })
}

/// The first candidate, which decides preflight and local-address policy; attempts retain the tail.
pub fn first(candidates: &[Target]) -> Result<&Target, Error> {
    candidates.first().ok_or_else(|| Error::Input {
        message: "target resolution returned no candidates".to_owned(),
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn a_next_hop_does_not_replace_the_request_scheme_or_transport() {
        let uri =
            Uri::parse(Bytes::from_static(b"sips:alice@example.test;transport=tcp")).expect("URI");
        let route = route_uri(&uri, "proxy.test:7443").expect("route");
        assert!(route.scheme().is_secure());
        assert_eq!(
            route.host().map(ToString::to_string).as_deref(),
            Some("proxy.test")
        );
        assert_eq!(route.port(), Some(7443));
        assert_eq!(route.transport(), Some(b"tcp".as_slice()));
    }

    #[test]
    fn a_next_hop_without_a_port_retains_the_uri_port() {
        let uri = Uri::parse(Bytes::from_static(b"sip:alice@example.test:5090")).expect("URI");
        let route = route_uri(&uri, "proxy.test").expect("route");
        assert_eq!(route.port(), Some(5090));
    }

    #[test]
    fn an_ipv6_next_hop_is_rendered_with_brackets() {
        let uri = Uri::parse(Bytes::from_static(b"sip:alice@example.test")).expect("URI");
        let route = route_uri(&uri, "[2001:db8::1]:5090").expect("route");
        assert_eq!(
            route.host().map(ToString::to_string).as_deref(),
            Some("[2001:db8::1]")
        );
        assert_eq!(route.port(), Some(5090));
    }

    #[tokio::test]
    async fn a_literal_does_not_need_system_resolver_setup() {
        let resolver = Resolver {
            dns: Err("deliberately unavailable".to_owned()),
            policy: ResolutionPolicy::default(),
        };
        let uri = Uri::parse(Bytes::from_static(b"sip:alice@192.0.2.8:5090")).expect("URI");

        let targets = resolver
            .resolve(&uri, None, None)
            .await
            .expect("literal target");
        assert_eq!(
            targets.first().map(|target| target.addr.to_string()),
            Some("192.0.2.8:5090".to_owned())
        );
    }

    /// The identity rule, at the case where it is most easily lost: a request that travels
    /// through a next hop is verified against the host it was addressed to, not the one it went
    /// through. A literal next hop needs no zone, so this is the whole rule with no DNS in it.
    #[tokio::test]
    async fn a_next_hop_does_not_become_the_verification_identity() {
        let resolver = Resolver {
            dns: Err("deliberately unavailable".to_owned()),
            policy: ResolutionPolicy::default(),
        };
        let uri = Uri::parse(Bytes::from_static(b"sips:alice@pbx.example")).expect("URI");

        let targets = resolver
            .resolve(&uri, Some("192.0.2.8:5061"), None)
            .await
            .expect("literal next hop");
        let first = targets.first().expect("a candidate");
        assert_eq!(first.addr.to_string(), "192.0.2.8:5061");
        assert!(first.transport.is_secure(), "sips: never downgrades");
        assert_eq!(first.verify_as.as_deref(), Some("pbx.example"));
    }

    /// The clamp, at the two boundaries that matter: a budget wider than the default bounds
    /// changes nothing, and a narrower one becomes both of them.
    #[test]
    fn a_budget_is_a_ceiling_over_both_resolution_deadlines() {
        let default = ResolutionPolicy::default();

        let generous = Resolver::within(Some(Duration::from_secs(60)));
        assert_eq!(
            generous.policy.resolution_timeout,
            default.resolution_timeout
        );
        assert_eq!(generous.policy.lookup_timeout, default.lookup_timeout);

        let tight = Resolver::within(Some(Duration::from_millis(500)));
        assert_eq!(tight.policy.resolution_timeout, Duration::from_millis(500));
        assert_eq!(tight.policy.lookup_timeout, Duration::from_millis(500));

        let stated_none = Resolver::within(None);
        assert_eq!(
            stated_none.policy.resolution_timeout,
            default.resolution_timeout
        );

        // Derived from a resolver rather than from the process: same client, tighter policy.
        let derived = stated_none.narrowed(Some(Duration::from_secs(1)));
        assert_eq!(derived.policy.resolution_timeout, Duration::from_secs(1));
        assert_eq!(derived.policy.lookup_timeout, Duration::from_secs(1));
        assert_eq!(
            stated_none.policy.resolution_timeout, default.resolution_timeout,
            "narrowing produces a resolver rather than changing the one it came from"
        );
    }

    #[test]
    fn a_nameserver_override_may_leave_the_port_implied() {
        assert_eq!(
            nameserver("192.0.2.53"),
            Some("192.0.2.53:53".parse().expect("a socket address"))
        );
        assert_eq!(
            nameserver("127.0.0.1:5353"),
            Some("127.0.0.1:5353".parse().expect("a socket address"))
        );
        assert_eq!(
            nameserver("[2001:db8::53]:5353"),
            Some("[2001:db8::53]:5353".parse().expect("a socket address"))
        );
    }

    /// Refused rather than ignored. A resolver override that silently falls back to the host's own
    /// nameservers would answer from a zone the caller did not ask about, and say nothing — and an
    /// unset shell variable expands to exactly the empty case.
    #[test]
    fn a_nameserver_override_that_cannot_be_read_is_refused() {
        assert_eq!(nameserver(""), None);
        assert_eq!(nameserver("resolver.example"), None);
        assert_eq!(nameserver("192.0.2.53:"), None);
    }

    #[test]
    fn resolution_deadlines_and_failures_are_classified_apart() {
        assert_eq!(
            Error::Resolution(ResolutionError::ResolutionTimeout).kind(),
            Kind::Timeout
        );
        assert_eq!(
            Error::Resolution(ResolutionError::LookupTimeout {
                query: "A/AAAA example.test".to_owned()
            })
            .kind(),
            Kind::Timeout
        );
        assert_eq!(
            Error::Resolution(ResolutionError::LookupUnavailable {
                query: "A/AAAA example.test".to_owned()
            })
            .kind(),
            Kind::Resolution
        );
        assert_eq!(
            Error::Input {
                message: "bad target".to_owned()
            }
            .kind(),
            Kind::Input
        );
        assert_eq!(
            Error::Setup {
                message: "no nameservers".to_owned()
            }
            .kind(),
            Kind::Setup
        );
    }
}
