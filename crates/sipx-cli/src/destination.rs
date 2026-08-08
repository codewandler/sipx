//! Shared outbound SIP destination resolution for command adapters.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use sipx_sip::{Host, Uri};
use sipx_transport::Target;
use sipx_transport::dns::{DnsResolver, ResolutionError, ResolutionPolicy, resolve_uri_bounded};
use sipx_transport::resolve::OsRng;

use crate::output::Exit;

/// T-38's finite serial connection-attempt budget.
pub(crate) const MAX_ATTEMPTS: usize = 16;

/// The nameserver to ask, when it should not be the one the host is configured with.
///
/// A phone that names a destination and cannot say *which* resolver it asked cannot tell a zone
/// that is wrong from a resolver that is unreachable — and those have different owners and
/// different fixes. Pointing it somewhere specific is also the only way the process tests can make
/// a resolution failure, a resolution deadline and a refused connection happen on purpose.
const NAMESERVER: &str = "SIPX_NAMESERVER";

/// Port 53, when `SIPX_NAMESERVER` names an address and not a port (RFC 1035 §4.2).
const DNS_PORT: u16 = 53;

/// A reusable system resolver. One command invocation gets one cache and one finite policy.
#[derive(Debug)]
pub(crate) struct Resolver {
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

    /// A system resolver whose own deadlines fit inside a command's remaining budget.
    ///
    /// `T-38`/`T-39` already bound resolution: one deadline per question and one over the whole
    /// of it. A command that also has a deadline over the *attempt* must not start a second clock
    /// beside those — a resolver still entitled to eight seconds inside a two-second attempt
    /// answers after the answer stopped being wanted, and both bounds would then be honest about
    /// different things. So the budget is pushed down into the same policy rather than raced
    /// against it: neither resolution deadline may exceed what the attempt has left.
    ///
    /// `None` is a command that states no deadline at all, and `T-38`'s own two bounds are then
    /// the whole of it. It is spelled here rather than at the call sites so that a command which
    /// gains a deadline later cannot keep an unbounded resolver by forgetting to say so.
    pub(crate) fn within(budget: Option<Duration>) -> Self {
        Self::system().narrowed(budget)
    }

    /// The same nameserver client, and the same cache, under one attempt's budget.
    ///
    /// `scenario` is one process placing many calls, each with its own deadline; building a client
    /// per `dial` would throw away what the previous lookups established — including the negative
    /// answers — between calls the actor is expected to place back to back.
    pub(crate) fn narrowed(&self, budget: Option<Duration>) -> Self {
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

    /// Resolve the request URI or an explicit next hop without changing the request URI.
    pub(crate) async fn resolve(
        &self,
        uri: &Uri,
        next_hop: Option<&str>,
        selection: crate::signalling::Selection,
        options: &crate::cli::SignallingOptions,
    ) -> Result<Vec<Target>, Error> {
        let route = match next_hop {
            Some(raw) => route_uri(uri, raw)?,
            None => uri.clone(),
        };
        let identity = uri.host().map(ToString::to_string).unwrap_or_default();
        let mut rng = OsRng;
        let candidates = if matches!(route.host(), Some(Host::Ip(_))) {
            sipx_transport::resolve_bounded(
                &route,
                &NoDns,
                &mut rng,
                selection.requested(),
                self.policy.limits,
            )
            .map_err(|error| Error::Resolution(ResolutionError::Selection(error)))?
        } else {
            let dns = self.dns.as_ref().map_err(|message| Error::Setup {
                message: message.clone(),
            })?;
            resolve_uri_bounded(&route, dns, &mut rng, selection.requested(), self.policy)
                .await
                .map_err(Error::Resolution)?
        };

        candidates
            .into_iter()
            .map(|target| selection.resolved_target(options, target, &identity))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|message| Error::Input { message })
    }
}

/// Build the DNS client the whole invocation shares.
///
/// The nameserver override is read here rather than parsed as a flag: it belongs to the machine
/// the command runs on, not to the call being placed, and every outbound command already routes
/// through this one constructor.
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
    // sipx's own. `Resolver::narrowed` only ever lowers those, so this stays true under a command
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

struct NoDns;

impl sipx_transport::Resolver for NoDns {
    fn naptr(&self, _domain: &str) -> Vec<sipx_transport::Naptr> {
        Vec::new()
    }

    fn srv(&self, _name: &str) -> Vec<sipx_transport::Srv> {
        Vec::new()
    }

    fn addresses(&self, _host: &str) -> Vec<std::net::IpAddr> {
        Vec::new()
    }
}

/// A failure that keeps usage, DNS failure and deadline exits distinct.
#[derive(Debug)]
pub(crate) enum Error {
    Input { message: String },
    Setup { message: String },
    Resolution(ResolutionError),
}

impl Error {
    pub(crate) fn exit(&self) -> Exit {
        match self {
            Self::Input { .. } => Exit::Usage,
            Self::Setup { .. } => Exit::Failed,
            Self::Resolution(error) => match error {
                ResolutionError::InvalidTimeout { .. }
                | ResolutionError::Selection(
                    sipx_transport::ResolutionError::InvalidLimit { .. }
                    | sipx_transport::ResolutionError::InvalidTransport
                    | sipx_transport::ResolutionError::ConflictingTransport { .. }
                    | sipx_transport::ResolutionError::SecureTransportRequired { .. },
                ) => Exit::Usage,
                ResolutionError::LookupTimeout { .. } | ResolutionError::ResolutionTimeout => {
                    Exit::Timeout
                }
                ResolutionError::LookupUnavailable { .. } | ResolutionError::Selection(_) | _ => {
                    Exit::Failed
                }
            },
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Input { message } => formatter.write_str(message),
            Self::Setup { message } => write!(formatter, "DNS resolver setup failed: {message}"),
            Self::Resolution(error) => write!(formatter, "target resolution failed: {error}"),
        }
    }
}

/// Construct the discovery URI for `--target`, retaining scheme, URI port and transport policy.
fn route_uri(uri: &Uri, raw: &str) -> Result<Uri, Error> {
    let (host, next_hop_port) = Host::parse_hostport(&Bytes::copy_from_slice(raw.as_bytes()))
        .map_err(|_| Error::Input {
            message: format!("invalid --target host or host:port: {raw}"),
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
        message: format!("invalid --target host or host:port: {raw}"),
    })
}

/// The first candidate determines preflight and local-address policy; attempts retain the tail.
pub(crate) fn first(candidates: &[Target]) -> Result<&Target, Error> {
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
        let options = crate::cli::SignallingOptions::default();
        let selection =
            crate::signalling::Selection::from_options(&options, false).expect("default transport");

        let targets = resolver
            .resolve(&uri, None, selection, &options)
            .await
            .expect("literal target");
        assert_eq!(
            targets.first().map(|target| target.addr.to_string()),
            Some("192.0.2.8:5090".to_owned())
        );
    }

    #[test]
    fn outbound_command_inventory_uses_the_shared_resolver() {
        for (name, source) in [
            ("dial", include_str!("dial.rs")),
            ("register", include_str!("register.rs")),
            ("load", include_str!("load.rs")),
            ("peers", include_str!("peers.rs")),
            ("scenario", include_str!("scenario.rs")),
        ] {
            assert!(
                source.contains(".resolve("),
                "outbound command {name} bypasses destination::Resolver"
            );
            // `P-26`: the budget is not optional at the call site. `system()` is private for this
            // reason, and naming it here keeps the reason findable from the command that would
            // have wanted it.
            assert!(
                source.contains("Resolver::within(") || source.contains(".narrowed("),
                "outbound command {name} resolves without stating the deadline it resolves under"
            );
        }
    }

    /// The clamp, at the two boundaries that matter: a budget wider than `T-38`'s own bounds
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
    fn resolution_deadlines_and_failures_have_distinct_exits() {
        let timeout = Error::Resolution(ResolutionError::ResolutionTimeout);
        let failure = Error::Resolution(ResolutionError::LookupUnavailable {
            query: "A/AAAA example.test".to_owned(),
        });
        let usage = Error::Input {
            message: "bad target".to_owned(),
        };
        assert_eq!(timeout.exit(), Exit::Timeout);
        assert_eq!(failure.exit(), Exit::Failed);
        assert_eq!(usage.exit(), Exit::Usage);
    }
}
