//! A real DNS client behind the RFC 3263 [`Resolver`] trait.
//!
//! [`crate::resolve()`] implements every selection rule and knows nothing about DNS; this is the
//! part that actually asks. The split is what lets the selection logic — where the bugs are —
//! be tested against fixtures with no network.
//!
//! Two things here are not just plumbing.
//!
//! **A failure is not an empty answer.** "There is no such record" is a final answer and the
//! next candidate should be tried; "the resolver did not respond" is a transient condition and
//! retrying later is right. Conflating them turns a thirty-second DNS blip into a permanent
//! routing failure, because the negative gets cached and nothing ever asks again.
//!
//! **Nothing here may block the endpoint loop.** A lookup can take seconds; the loop it would
//! block owns every transaction timer.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hickory_resolver::TokioResolver;
use hickory_resolver::config::{
    ConnectionConfig, NameServerConfig, ProtocolConfig, ResolverConfig, ResolverOpts,
};
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hickory_resolver::proto::rr::rdata::{NAPTR, SRV};
use hickory_resolver::proto::rr::{RData, RecordType};
use tokio::sync::Mutex;

use crate::resolve::{Naptr, Resolver, Srv};

/// What a lookup produced.
///
/// The distinction between "no records" and "could not ask" is the whole reason this is not
/// just `Vec<T>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer<T> {
    /// The server answered, with these records — possibly none.
    Records(Vec<T>),
    /// The question could not be asked or answered. Not a statement about the name.
    Unavailable,
}

impl<T> Answer<T> {
    /// The records, treating an unavailable server as empty.
    ///
    /// Used at the boundary where the [`Resolver`] trait cannot express the difference. Named
    /// rather than implicit, so the loss of information is visible at the call site.
    pub fn or_empty(self) -> Vec<T> {
        match self {
            Self::Records(records) => records,
            Self::Unavailable => Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct Cached<T> {
    records: Vec<T>,
    expires: Instant,
}

/// A DNS-backed resolver with a TTL-respecting cache.
#[derive(Debug)]
pub struct DnsResolver {
    inner: TokioResolver,
    naptr: Mutex<std::collections::HashMap<String, Cached<Naptr>>>,
    srv: Mutex<std::collections::HashMap<String, Cached<Srv>>>,
    addresses: Mutex<std::collections::HashMap<String, Cached<IpAddr>>>,
    addresses_v6: Mutex<std::collections::HashMap<String, Cached<IpAddr>>>,
    /// Never cache for longer than this, however generous the TTL.
    max_ttl: Duration,
}

impl DnsResolver {
    /// A resolver using the system's configured nameservers.
    pub fn from_system() -> std::io::Result<Self> {
        let builder = TokioResolver::builder_tokio()
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let resolver = builder
            .build()
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        Ok(Self::with(resolver))
    }

    /// A resolver pointed at specific nameservers.
    ///
    /// This is what makes the tests possible: they run a fixture DNS server on localhost and
    /// point sipx at it, rather than asking the public internet and hoping.
    pub fn with_config(config: ResolverConfig, options: ResolverOpts) -> std::io::Result<Self> {
        let mut builder =
            TokioResolver::builder_with_config(config, TokioRuntimeProvider::default());
        *builder.options_mut() = options;
        let resolver = builder
            .build()
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        Ok(Self::with(resolver))
    }

    fn with(inner: TokioResolver) -> Self {
        Self {
            inner,
            naptr: Mutex::new(std::collections::HashMap::new()),
            srv: Mutex::new(std::collections::HashMap::new()),
            addresses: Mutex::new(std::collections::HashMap::new()),
            addresses_v6: Mutex::new(std::collections::HashMap::new()),
            max_ttl: Duration::from_secs(3600),
        }
    }

    /// A resolver pointed at one nameserver.
    ///
    /// Wraps the DNS client's own configuration types so they appear in exactly one place. They
    /// change shape between releases — this file has been through one such change already —
    /// and a caller of sipx should not have to track that to point a resolver somewhere.
    pub fn for_nameserver(server: SocketAddr, timeout: Duration) -> std::io::Result<Self> {
        let mut connection = ConnectionConfig::new(ProtocolConfig::Udp);
        connection.port = server.port();

        let name_server = NameServerConfig::new(server.ip(), true, vec![connection]);
        let config = ResolverConfig::from_parts(None, Vec::new(), vec![name_server]);

        let mut options = ResolverOpts::default();
        options.timeout = timeout;
        // The fixture zone in the tests is authoritative for names that do not exist anywhere
        // else, and a hosts file entry would silently win over it.
        options.use_hosts_file = hickory_resolver::config::ResolveHosts::Never;
        // The client has a response cache of its own. Two caches with different TTL policies
        // is a source of confusion rather than of speed — sipx's exists to cap TTLs and to
        // distinguish "no such record" from "could not ask", and neither of those survives a
        // layer underneath doing its own thing. One cache, in one place.
        options.cache_size = 0;

        Self::with_config(config, options)
    }

    /// Cap how long any record is held, however long its TTL claims.
    #[must_use]
    pub fn with_max_ttl(mut self, max_ttl: Duration) -> Self {
        self.max_ttl = max_ttl;
        self
    }

    /// NAPTR records for a domain.
    pub async fn naptr(&self, domain: &str) -> Answer<Naptr> {
        self.lookup(
            &self.naptr,
            domain,
            RecordType::NAPTR,
            |record| match &record.data {
                RData::NAPTR(naptr) => Some(convert_naptr(naptr)),
                _ => None,
            },
        )
        .await
    }

    /// SRV records for a name.
    pub async fn srv(&self, name: &str) -> Answer<Srv> {
        self.lookup(&self.srv, name, RecordType::SRV, |record| {
            match &record.data {
                RData::SRV(srv) => Some(convert_srv(srv)),
                _ => None,
            }
        })
        .await
    }

    /// Addresses for a host.
    ///
    /// Both families, because RFC 3263 does not distinguish them and a host with only an AAAA
    /// record is reachable.
    pub async fn addresses(&self, host: &str) -> Answer<IpAddr> {
        let v4 = self
            .lookup(
                &self.addresses,
                host,
                RecordType::A,
                |record| match &record.data {
                    RData::A(a) => Some(IpAddr::V4(a.0)),
                    _ => None,
                },
            )
            .await;

        // A host with no A record may still have AAAA; asking only for A would make it
        // unreachable for a reason nothing reports.
        match v4 {
            Answer::Records(records) if !records.is_empty() => Answer::Records(records),
            other => {
                let v6 = self
                    .lookup(
                        &self.addresses_v6,
                        host,
                        RecordType::AAAA,
                        |record| match &record.data {
                            RData::AAAA(aaaa) => Some(IpAddr::V6(aaaa.0)),
                            _ => None,
                        },
                    )
                    .await;
                match (other, v6) {
                    // Only report "the server answered, with nothing" if both did.
                    (Answer::Records(_), Answer::Records(records)) => Answer::Records(records),
                    (_, Answer::Records(records)) if !records.is_empty() => {
                        Answer::Records(records)
                    }
                    _ => Answer::Unavailable,
                }
            }
        }
    }

    /// One cached lookup, shared by all three record types.
    ///
    /// **Concurrent callers asking for the same name make one query**, which is what a forwarding
    /// element needs: it resolves for every call it forwards, and a burst to one domain arrives
    /// together, so without coalescing one cache miss becomes one query per concurrent call — every
    /// one of them missing the cache because none has finished yet.
    ///
    /// That coalescing is **not implemented here**. `hickory-resolver` already does it, and a
    /// single-flight layer written on top of it was measured to change nothing: eight concurrent
    /// lookups reach the fixture nameserver exactly once with or without it, so the layer was
    /// removed rather than kept as decoration. The property is load-bearing all the same, so
    /// `two_concurrent_resolutions_of_one_name_make_one_query` pins it — if the client is ever
    /// swapped or configured differently, that test is what notices.
    async fn lookup<T: Clone>(
        &self,
        cache: &Mutex<std::collections::HashMap<String, Cached<T>>>,
        name: &str,
        record_type: RecordType,
        extract: impl Fn(&hickory_resolver::proto::rr::Record) -> Option<T>,
    ) -> Answer<T> {
        if let Some(records) = cached(cache, name).await {
            return Answer::Records(records);
        }

        let lookup = match self.inner.lookup(name, record_type).await {
            Ok(lookup) => lookup,
            Err(error) => {
                let answer = classify(&error);
                // RFC 2308: a *genuine* negative answer is cacheable, for the TTL the zone's SOA
                // states. Caching it is the difference between one query per absent name and one
                // per call to it — a domain with no `_sips._tcp` record is asked about on every
                // single call otherwise, which is what a forwarding element does thousands of
                // times a minute.
                //
                // `Unavailable` is deliberately *not* cached. It means nothing answered, and
                // remembering a network blip as a routing decision would keep a domain
                // unreachable long after it came back.
                if matches!(answer, Answer::Records(_)) {
                    store(cache, name, &[], negative_ttl(&error, self.max_ttl)).await;
                }
                return answer;
            }
        };

        let answers = lookup.answers();
        let ttl = shortest_ttl(answers.iter().map(|record| record.ttl), self.max_ttl);
        let records: Vec<T> = answers.iter().filter_map(&extract).collect();
        store(cache, name, &records, ttl).await;
        Answer::Records(records)
    }
}

/// Whether an error means "no such record" or "could not ask".
///
/// Harder than it should be. The client reports an unreachable nameserver as `NoRecordsFound`
/// with response code `NXDomain` — the same shape as a real negative answer — so the error kind
/// alone cannot tell them apart.
///
/// The signal that does distinguish them is RFC 2308's: a genuine negative answer carries the
/// zone's SOA record, because that is what tells a resolver how long to cache the negative.
/// A synthesised one has no SOA and no negative TTL, because no server ever said anything.
///
/// This errs toward `Unavailable`: a real negative answer from a server that omits the SOA is
/// treated as "could not ask", so sipx retries instead of falling through. That is the milder
/// error — retrying a name that genuinely does not exist costs a lookup, while caching a
/// network blip as a routing decision costs every call to that domain until something evicts
/// it.
fn classify<T>(error: &hickory_resolver::net::NetError) -> Answer<T> {
    use hickory_resolver::net::{DnsError, NetError};

    if let NetError::Dns(DnsError::NoRecordsFound(no_records)) = error {
        let answered = no_records.soa.is_some() || no_records.negative_ttl.is_some();
        return if answered {
            Answer::Records(Vec::new())
        } else {
            Answer::Unavailable
        };
    }
    Answer::Unavailable
}

/// How long a negative answer may be remembered (RFC 2308 §5).
///
/// The SOA's TTL is what the zone says about its own absences. Capped the same way a positive
/// answer is, and floored at nothing — a zone that says zero means "ask every time", and obeying
/// that is cheaper than arguing with it.
fn negative_ttl(error: &hickory_resolver::net::NetError, max: Duration) -> Duration {
    use hickory_resolver::net::{DnsError, NetError};
    if let NetError::Dns(DnsError::NoRecordsFound(no_records)) = error
        && let Some(ttl) = no_records.negative_ttl
    {
        return Duration::from_secs(u64::from(ttl)).min(max);
    }
    // A negative answer whose SOA carried no explicit TTL. Short, because guessing long about an
    // absence is how a record that has just been created stays invisible.
    Duration::from_secs(30).min(max)
}

/// The shortest TTL in a set, which is how long the whole set may be held.
///
/// The shortest rather than the longest: holding a record past its TTL is the failure mode
/// that matters, because it points traffic at a server that has moved.
fn shortest_ttl(ttls: impl Iterator<Item = u32>, max: Duration) -> Duration {
    ttls.min()
        .map_or(max, |ttl| Duration::from_secs(u64::from(ttl)).min(max))
}

async fn cached<T: Clone>(
    map: &Mutex<std::collections::HashMap<String, Cached<T>>>,
    key: &str,
) -> Option<Vec<T>> {
    let guard = map.lock().await;
    let entry = guard.get(key)?;
    // An expired entry is not returned, and is not preferred over asking again.
    (entry.expires > Instant::now()).then(|| entry.records.clone())
}

async fn store<T: Clone>(
    map: &Mutex<std::collections::HashMap<String, Cached<T>>>,
    key: &str,
    records: &[T],
    ttl: Duration,
) {
    map.lock().await.insert(
        key.to_owned(),
        Cached {
            records: records.to_vec(),
            expires: Instant::now() + ttl,
        },
    );
}

fn convert_naptr(naptr: &NAPTR) -> Naptr {
    Naptr {
        order: naptr.order,
        preference: naptr.preference,
        service: String::from_utf8_lossy(&naptr.services).into_owned(),
        replacement: strip_root(&naptr.replacement.to_string()),
    }
}

fn convert_srv(srv: &SRV) -> Srv {
    Srv {
        priority: srv.priority,
        weight: srv.weight,
        port: srv.port,
        target: strip_root(&srv.target.to_string()),
    }
}

/// DNS names are absolute and end in a dot; the rest of sipx works in the relative form the
/// URI used. Leaving the dot on turns `_sip._udp.example.com.` into a name no fixture matches.
fn strip_root(name: &str) -> String {
    name.strip_suffix('.').unwrap_or(name).to_owned()
}

/// Blocking adapter so [`DnsResolver`] can satisfy the synchronous [`Resolver`] trait.
///
/// The trait is synchronous because RFC 3263 selection is pure computation over records. Doing
/// the lookups up front and handing over the results keeps it that way — and keeps every
/// await off the endpoint loop, where a slow resolver would stop the transaction timers.
#[derive(Debug, Clone)]
pub struct Prefetched {
    naptr: Vec<Naptr>,
    srv: std::collections::HashMap<String, Vec<Srv>>,
    addresses: std::collections::HashMap<String, Vec<IpAddr>>,
}

impl Prefetched {
    /// Ask for everything RFC 3263 could need for one URI, then hand the answers to the
    /// selection logic.
    pub async fn for_domain(resolver: &Arc<DnsResolver>, domain: &str) -> Self {
        let naptr = resolver.naptr(domain).await.or_empty();

        // Every SRV name the NAPTR records point at, plus the conventional ones, since a
        // domain with no NAPTR may still have SRV.
        let mut srv_names: Vec<String> = naptr.iter().map(|n| n.replacement.clone()).collect();
        // Every prefix RFC 3263 §4.1 and RFC 7118 §6 define, not only the three a phone uses. A
        // WebSocket destination whose prefix is missing here is not unreachable — it just pays a
        // serial lookup later, one round trip at a time, which is exactly what prefetching exists
        // to avoid.
        for prefix in [
            "_sip._udp.",
            "_sip._tcp.",
            "_sips._tcp.",
            "_sip._ws.",
            "_sips._wss.",
        ] {
            srv_names.push(format!("{prefix}{domain}"));
        }
        srv_names.sort_unstable();
        srv_names.dedup();

        let mut srv = std::collections::HashMap::new();
        let mut hosts = vec![domain.to_owned()];
        for name in srv_names {
            let records = resolver.srv(&name).await.or_empty();
            hosts.extend(records.iter().map(|r| r.target.clone()));
            srv.insert(name, records);
        }

        hosts.sort_unstable();
        hosts.dedup();
        let mut addresses = std::collections::HashMap::new();
        for host in hosts {
            let found = resolver.addresses(&host).await.or_empty();
            addresses.insert(host, found);
        }

        Self {
            naptr,
            srv,
            addresses,
        }
    }
}

/// Resolve a URI to an ordered candidate list, in one await (RFC 3263).
///
/// The two-step form — prefetch, then select — is what the endpoint uses, because it keeps every
/// await off the loop where a slow nameserver would stop the transaction timers. This is the same
/// thing for a caller that is not the loop: a forwarding element deciding where to send one
/// request, which wants a list and does not want to know that resolution has two halves.
///
/// The selection is unchanged and still pure. Only the waiting is here.
pub async fn resolve_uri<G: crate::resolve::Rng + ?Sized>(
    uri: &sipx_sip::Uri,
    resolver: &Arc<DnsResolver>,
    rng: &mut G,
) -> Vec<crate::Target> {
    let Some(domain) = uri.host().map(ToString::to_string) else {
        return Vec::new();
    };
    let prefetched = Prefetched::for_domain(resolver, &domain).await;
    crate::resolve::resolve(uri, &prefetched, rng)
}

impl Resolver for Prefetched {
    fn naptr(&self, _domain: &str) -> Vec<Naptr> {
        self.naptr.clone()
    }

    fn srv(&self, name: &str) -> Vec<Srv> {
        self.srv.get(name).cloned().unwrap_or_default()
    }

    fn addresses(&self, host: &str) -> Vec<IpAddr> {
        self.addresses.get(host).cloned().unwrap_or_default()
    }
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

    #[test]
    fn the_root_dot_is_stripped_from_dns_names() {
        assert_eq!(
            strip_root("_sip._udp.example.com."),
            "_sip._udp.example.com"
        );
        assert_eq!(strip_root("example.com"), "example.com");
        assert_eq!(strip_root(""), "");
    }

    /// The shortest TTL governs the set: holding a record past its TTL points traffic at a
    /// server that has moved, which is the failure that matters.
    #[test]
    fn the_shortest_ttl_governs_the_set() {
        let max = Duration::from_secs(3600);
        assert_eq!(
            shortest_ttl([300u32, 60, 900].into_iter(), max),
            Duration::from_secs(60)
        );
        assert_eq!(
            shortest_ttl([7200u32].into_iter(), max),
            max,
            "a generous TTL is still capped"
        );
        assert_eq!(
            shortest_ttl(std::iter::empty(), max),
            max,
            "no records means the cap"
        );
    }

    /// The distinction this module exists for, at the type level: an unavailable server is not
    /// an empty answer, and collapsing the two has to be explicit.
    #[test]
    fn an_unavailable_server_is_not_an_empty_answer() {
        let empty: Answer<Srv> = Answer::Records(Vec::new());
        let down: Answer<Srv> = Answer::Unavailable;
        assert_ne!(empty, down);
        assert!(
            down.or_empty().is_empty(),
            "collapsing is possible, but named"
        );
    }

    #[tokio::test]
    async fn a_resolver_that_cannot_reach_a_server_reports_unavailable_not_empty() {
        // A nameserver on a port nothing listens on, with a short timeout.
        let resolver = DnsResolver::for_nameserver(
            "127.0.0.1:9".parse().expect("valid"),
            Duration::from_millis(200),
        )
        .expect("builds");
        assert_eq!(
            resolver.srv("_sip._udp.example.invalid").await,
            Answer::Unavailable,
            "a dead nameserver must not look like 'no such record'"
        );
    }

    /// A cached entry that has expired is not preferred over asking again.
    #[tokio::test]
    async fn an_expired_entry_is_not_returned() {
        let map: Mutex<std::collections::HashMap<String, Cached<Srv>>> =
            Mutex::new(std::collections::HashMap::new());
        let record = Srv {
            priority: 1,
            weight: 0,
            port: 5060,
            target: "host.example".to_owned(),
        };

        store(
            &map,
            "name",
            std::slice::from_ref(&record),
            Duration::from_millis(50),
        )
        .await;
        assert_eq!(cached(&map, "name").await, Some(vec![record]));

        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(
            cached(&map, "name").await,
            None,
            "an expired entry must be re-asked, not served"
        );
    }

    #[tokio::test]
    async fn a_fresh_entry_is_served_from_cache() {
        let map: Mutex<std::collections::HashMap<String, Cached<IpAddr>>> =
            Mutex::new(std::collections::HashMap::new());
        let address: IpAddr = "192.0.2.1".parse().expect("valid");
        store(&map, "host", &[address], Duration::from_secs(60)).await;
        assert_eq!(cached(&map, "host").await, Some(vec![address]));
    }
}
