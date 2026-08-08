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

/// DNS waiting policy paired with the pure selection limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolutionPolicy {
    /// Pure lookup, record and candidate limits.
    pub limits: crate::ResolutionLimits,
    /// Maximum wait for one DNS question.
    pub lookup_timeout: Duration,
    /// Maximum wait for every DNS question and pure selection together.
    pub resolution_timeout: Duration,
}

impl Default for ResolutionPolicy {
    fn default() -> Self {
        Self {
            limits: crate::ResolutionLimits::default(),
            lookup_timeout: Duration::from_secs(2),
            resolution_timeout: Duration::from_secs(8),
        }
    }
}

/// Why the DNS-backed resolution adapter did not produce candidates.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ResolutionError {
    /// Pure selection refused the values or a finite count.
    #[error(transparent)]
    Selection(#[from] crate::ResolutionError),
    /// A deadline cannot bound work.
    #[error("resolution timeout {field} must be greater than zero")]
    InvalidTimeout {
        /// Public timeout field.
        field: &'static str,
    },
    /// A question produced no trustworthy positive or negative answer.
    #[error("DNS lookup unavailable for {query}")]
    LookupUnavailable {
        /// Exact question name.
        query: String,
    },
    /// One question exceeded its deadline.
    #[error("DNS lookup timed out for {query}")]
    LookupTimeout {
        /// Exact question name.
        query: String,
    },
    /// The complete resolution exceeded its deadline.
    #[error("SIP target resolution timed out")]
    ResolutionTimeout,
}

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
enum CachedRecords {
    Naptr(Vec<Naptr>),
    Srv(Vec<Srv>),
    Addresses(Vec<IpAddr>),
}

trait CacheRecord: Clone {
    fn read(records: &CachedRecords) -> Option<Vec<Self>>;
    fn store(records: &[Self]) -> CachedRecords;
}

impl CacheRecord for Naptr {
    fn read(records: &CachedRecords) -> Option<Vec<Self>> {
        match records {
            CachedRecords::Naptr(records) => Some(records.clone()),
            CachedRecords::Srv(_) | CachedRecords::Addresses(_) => None,
        }
    }

    fn store(records: &[Self]) -> CachedRecords {
        CachedRecords::Naptr(records.to_vec())
    }
}

impl CacheRecord for Srv {
    fn read(records: &CachedRecords) -> Option<Vec<Self>> {
        match records {
            CachedRecords::Srv(records) => Some(records.clone()),
            CachedRecords::Naptr(_) | CachedRecords::Addresses(_) => None,
        }
    }

    fn store(records: &[Self]) -> CachedRecords {
        CachedRecords::Srv(records.to_vec())
    }
}

impl CacheRecord for IpAddr {
    fn read(records: &CachedRecords) -> Option<Vec<Self>> {
        match records {
            CachedRecords::Addresses(records) => Some(records.clone()),
            CachedRecords::Naptr(_) | CachedRecords::Srv(_) => None,
        }
    }

    fn store(records: &[Self]) -> CachedRecords {
        CachedRecords::Addresses(records.to_vec())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    name: String,
    record_type: RecordType,
}

#[derive(Debug, Clone)]
struct Cached {
    records: CachedRecords,
    expires: Instant,
    last_used: u64,
}

#[derive(Debug)]
struct Cache {
    entries: std::collections::HashMap<CacheKey, Cached>,
    capacity: usize,
    clock: u64,
}

impl Default for Cache {
    fn default() -> Self {
        Self {
            entries: std::collections::HashMap::new(),
            capacity: 1_024,
            clock: 0,
        }
    }
}

/// A DNS-backed resolver with a TTL-respecting cache.
#[derive(Debug)]
pub struct DnsResolver {
    inner: TokioResolver,
    cache: Mutex<Cache>,
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
            cache: Mutex::new(Cache::default()),
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

    /// Bound the total number of positive and negative entries across every record type.
    ///
    /// Zero disables sipx's cache, which is useful for deterministic adapter tests.
    #[must_use]
    pub fn with_cache_capacity(mut self, capacity: usize) -> Self {
        self.cache.get_mut().capacity = capacity;
        self
    }

    /// NAPTR records for a domain.
    pub async fn naptr(&self, domain: &str) -> Answer<Naptr> {
        self.lookup(domain, RecordType::NAPTR, |record| match &record.data {
            RData::NAPTR(naptr) => Some(convert_naptr(naptr)),
            _ => None,
        })
        .await
    }

    /// SRV records for a name.
    pub async fn srv(&self, name: &str) -> Answer<Srv> {
        self.lookup(name, RecordType::SRV, |record| match &record.data {
            RData::SRV(srv) => Some(convert_srv(srv)),
            _ => None,
        })
        .await
    }

    /// Addresses for a host.
    ///
    /// Both families, because RFC 3263 does not distinguish them and a host with only an AAAA
    /// record is reachable.
    pub async fn addresses(&self, host: &str) -> Answer<IpAddr> {
        // RFC 7984 §3.1: a dual-stack client asks for every supported family. These are sibling
        // child futures, not tasks; cancelling `addresses` cancels both and leaves nothing detached.
        let (v6, v4) = tokio::join!(
            self.lookup(host, RecordType::AAAA, |record| match &record.data {
                RData::AAAA(aaaa) => Some(IpAddr::V6(aaaa.0)),
                _ => None,
            },),
            self.lookup(host, RecordType::A, |record| match &record.data {
                RData::A(a) => Some(IpAddr::V4(a.0)),
                _ => None,
            },),
        );

        match (v6, v4) {
            (Answer::Records(v6), Answer::Records(mut v4)) => {
                // Preserve the adapter's established IPv4-first order until address selection is
                // supplied by the caller. Every address remains adjacent within its host.
                v4.extend(v6);
                Answer::Records(v4)
            }
            (Answer::Records(records), Answer::Unavailable)
            | (Answer::Unavailable, Answer::Records(records))
                if !records.is_empty() =>
            {
                Answer::Records(records)
            }
            _ => Answer::Unavailable,
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
    /// `concurrent_resolutions_make_one_query_per_address_family` pins it — if the client is ever
    /// swapped or configured differently, that test is what notices.
    async fn lookup<T: CacheRecord>(
        &self,
        name: &str,
        record_type: RecordType,
        extract: impl Fn(&hickory_resolver::proto::rr::Record) -> Option<T>,
    ) -> Answer<T> {
        let key = CacheKey {
            name: name.to_owned(),
            record_type,
        };
        if let Some(records) = cached(&self.cache, &key).await {
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
                    store::<T>(&self.cache, key, &[], negative_ttl(&error, self.max_ttl)).await;
                }
                return answer;
            }
        };

        let answers = lookup.answers();
        let ttl = shortest_ttl(answers.iter().map(|record| record.ttl), self.max_ttl);
        let records: Vec<T> = answers.iter().filter_map(&extract).collect();
        store(&self.cache, key, &records, ttl).await;
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

async fn cached<T: CacheRecord>(cache: &Mutex<Cache>, key: &CacheKey) -> Option<Vec<T>> {
    let mut guard = cache.lock().await;
    let now = Instant::now();
    if guard
        .entries
        .get(key)
        .is_some_and(|entry| entry.expires <= now)
    {
        guard.entries.remove(key);
        return None;
    }
    let records = guard
        .entries
        .get(key)
        .and_then(|entry| T::read(&entry.records))?;
    guard.clock = guard.clock.saturating_add(1);
    let last_used = guard.clock;
    if let Some(entry) = guard.entries.get_mut(key) {
        entry.last_used = last_used;
    }
    Some(records)
}

async fn store<T: CacheRecord>(cache: &Mutex<Cache>, key: CacheKey, records: &[T], ttl: Duration) {
    let mut guard = cache.lock().await;
    if guard.capacity == 0 {
        return;
    }

    let now = Instant::now();
    guard.entries.retain(|_, entry| entry.expires > now);
    if !guard.entries.contains_key(&key) && guard.entries.len() >= guard.capacity {
        let evicted = guard
            .entries
            .iter()
            .min_by_key(|(key, entry)| {
                (
                    entry.last_used,
                    key.name.as_str(),
                    record_type_order(key.record_type),
                )
            })
            .map(|(key, _)| key.clone());
        if let Some(evicted) = evicted {
            guard.entries.remove(&evicted);
        }
    }
    guard.clock = guard.clock.saturating_add(1);
    let last_used = guard.clock;
    guard.entries.insert(
        key,
        Cached {
            records: T::store(records),
            expires: now + ttl,
            last_used,
        },
    );
}

fn record_type_order(record_type: RecordType) -> u16 {
    match record_type {
        RecordType::A => 1,
        RecordType::AAAA => 2,
        RecordType::NAPTR => 3,
        RecordType::SRV => 4,
        _ => u16::MAX,
    }
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
#[derive(Debug, Clone, Default)]
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

    async fn for_uri(
        resolver: &Arc<DnsResolver>,
        uri: &sipx_sip::Uri,
        requested_transport: Option<crate::TransportKind>,
        policy: ResolutionPolicy,
    ) -> Result<(sipx_sip::Uri, Self), ResolutionError> {
        if policy.lookup_timeout.is_zero() {
            return Err(ResolutionError::InvalidTimeout {
                field: "lookup_timeout",
            });
        }
        if policy.resolution_timeout.is_zero() {
            return Err(ResolutionError::InvalidTimeout {
                field: "resolution_timeout",
            });
        }

        let effective = crate::resolve::effective_uri(uri, requested_transport)?;
        let Some(sipx_sip::Host::Name(name)) = effective.host() else {
            return Ok((effective, Self::default()));
        };
        let domain = String::from_utf8_lossy(name.as_bytes()).into_owned();
        let mut lookups = 0usize;

        if effective.port().is_some() {
            claim_lookups(&mut lookups, 2, policy.limits)?;
            let records = address_records(resolver, &domain, policy.lookup_timeout).await?;
            check_record_count("addresses", records.len(), policy.limits.max_addresses)?;
            return Ok((
                effective,
                Self {
                    naptr: Vec::new(),
                    srv: std::collections::HashMap::new(),
                    addresses: std::collections::HashMap::from([(domain, records)]),
                },
            ));
        }

        let explicit_transport = effective.transport().is_some();
        let naptr = if explicit_transport {
            Vec::new()
        } else {
            claim_lookups(&mut lookups, 1, policy.limits)?;
            let records =
                answer_records(resolver.naptr(&domain), &domain, policy.lookup_timeout).await?;
            check_record_count(
                "NAPTR records",
                records.len(),
                policy.limits.max_naptr_records,
            )?;
            records
        };

        let mut srv_names: Vec<String> = if explicit_transport {
            let transport = uri_transport(&effective)?;
            vec![format!("{}{domain}", crate::resolve::srv_prefix(transport))]
        } else {
            let mut names: Vec<String> = naptr
                .iter()
                .map(|record| record.replacement.clone())
                .collect();
            if effective.scheme().is_secure() {
                names.push(format!("_sips._tcp.{domain}"));
            } else {
                names.extend([
                    format!("_sip._udp.{domain}"),
                    format!("_sip._tcp.{domain}"),
                    format!("_sips._tcp.{domain}"),
                ]);
            }
            names
        };
        srv_names.sort_unstable();
        srv_names.dedup();

        let mut srv = std::collections::HashMap::new();
        let mut hosts = vec![domain.clone()];
        for query in srv_names {
            claim_lookups(&mut lookups, 1, policy.limits)?;
            let records =
                answer_records(resolver.srv(&query), &query, policy.lookup_timeout).await?;
            check_record_count("SRV records", records.len(), policy.limits.max_srv_records)?;
            hosts.extend(records.iter().map(|record| record.target.clone()));
            srv.insert(query, records);
        }

        hosts.sort_unstable();
        hosts.dedup();
        let mut addresses = std::collections::HashMap::new();
        for host in hosts {
            claim_lookups(&mut lookups, 2, policy.limits)?;
            let records = address_records(resolver, &host, policy.lookup_timeout).await?;
            check_record_count("addresses", records.len(), policy.limits.max_addresses)?;
            addresses.insert(host, records);
        }

        Ok((
            effective,
            Self {
                naptr,
                srv,
                addresses,
            },
        ))
    }
}

fn claim_lookups(
    observed: &mut usize,
    amount: usize,
    limits: crate::ResolutionLimits,
) -> Result<(), ResolutionError> {
    *observed = observed.saturating_add(amount);
    if *observed > limits.max_lookups {
        return Err(crate::ResolutionError::LimitExceeded {
            limit: "lookups",
            maximum: limits.max_lookups,
            observed: *observed,
        }
        .into());
    }
    Ok(())
}

fn check_record_count(
    limit: &'static str,
    observed: usize,
    maximum: usize,
) -> Result<(), ResolutionError> {
    if observed > maximum {
        return Err(crate::ResolutionError::LimitExceeded {
            limit,
            maximum,
            observed,
        }
        .into());
    }
    Ok(())
}

async fn answer_records<T>(
    answer: impl std::future::Future<Output = Answer<T>>,
    query: &str,
    within: Duration,
) -> Result<Vec<T>, ResolutionError> {
    match tokio::time::timeout(within, answer).await {
        Ok(Answer::Records(records)) => Ok(records),
        Ok(Answer::Unavailable) => Err(ResolutionError::LookupUnavailable {
            query: query.to_owned(),
        }),
        Err(_) => Err(ResolutionError::LookupTimeout {
            query: query.to_owned(),
        }),
    }
}

async fn address_records(
    resolver: &Arc<DnsResolver>,
    host: &str,
    within: Duration,
) -> Result<Vec<IpAddr>, ResolutionError> {
    answer_records(resolver.addresses(host), &format!("A/AAAA {host}"), within).await
}

fn uri_transport(uri: &sipx_sip::Uri) -> Result<crate::TransportKind, ResolutionError> {
    let transport = uri
        .selected_transport()
        .map_err(|_| crate::ResolutionError::InvalidTransport)?;
    Ok(match transport {
        sipx_sip::UriTransport::Udp => crate::TransportKind::Udp,
        sipx_sip::UriTransport::Tcp => crate::TransportKind::Tcp,
        sipx_sip::UriTransport::Tls => crate::TransportKind::Tls,
        sipx_sip::UriTransport::Ws => crate::TransportKind::Ws,
        sipx_sip::UriTransport::Wss => crate::TransportKind::Wss,
        sipx_sip::UriTransport::Quic => crate::TransportKind::Quic,
    })
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

/// Resolve through the DNS adapter with finite question, record, candidate and wall-clock bounds.
///
/// No task is detached: the complete operation and every lookup are child futures of this await,
/// so cancelling the caller drops all active resolver work.
pub async fn resolve_uri_bounded<G: crate::resolve::Rng + ?Sized>(
    uri: &sipx_sip::Uri,
    resolver: &Arc<DnsResolver>,
    rng: &mut G,
    requested_transport: Option<crate::TransportKind>,
    policy: ResolutionPolicy,
) -> Result<Vec<crate::Target>, ResolutionError> {
    let operation = async {
        let (effective, prefetched) =
            Prefetched::for_uri(resolver, uri, requested_transport, policy).await?;
        crate::resolve::resolve_bounded(&effective, &prefetched, rng, None, policy.limits)
            .map_err(ResolutionError::from)
    };
    tokio::time::timeout(policy.resolution_timeout, operation)
        .await
        .map_err(|_| ResolutionError::ResolutionTimeout)?
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
#[cfg_attr(coverage_nightly, coverage(off))]
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
        let cache = Mutex::new(Cache::default());
        let key = CacheKey {
            name: "name".to_owned(),
            record_type: RecordType::SRV,
        };
        let record = Srv {
            priority: 1,
            weight: 0,
            port: 5060,
            target: "host.example".to_owned(),
        };

        // Two stores, because the two halves of this test want opposite things from the clock
        // (`X-29`). The read below is a *precondition* — it proves the entry was stored at all,
        // so the expiry half cannot pass by having stored nothing — and it must not race the
        // TTL. On 2026-07-29 it did: with a 50 ms TTL and three worktrees compiling, the entry
        // expired before this immediate read, and a gate for a diff that had never opened this
        // crate came back red. A minute is a bound on failure, not a window to measure in.
        store(
            &cache,
            key.clone(),
            std::slice::from_ref(&record),
            Duration::from_secs(60),
        )
        .await;
        assert_eq!(cached(&cache, &key).await, Some(vec![record.clone()]));

        // The expiry is then a real one — the same `Instant` comparison against a TTL that
        // genuinely elapses — but it is waited *for* rather than slept past. Load can only
        // lengthen the wait, and the deadline turns "never expires" into a failure that says so
        // rather than into a flake.
        store(
            &cache,
            key.clone(),
            std::slice::from_ref(&record),
            Duration::from_millis(50),
        )
        .await;
        let deadline = Instant::now() + Duration::from_secs(10);
        while cached::<Srv>(&cache, &key).await.is_some() {
            assert!(
                Instant::now() < deadline,
                "an entry with a 50 ms TTL never expired"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        assert_eq!(
            cached::<Srv>(&cache, &key).await,
            None,
            "an expired entry must be re-asked, not served"
        );
    }

    #[tokio::test]
    async fn a_fresh_entry_is_served_from_cache() {
        let cache = Mutex::new(Cache::default());
        let key = CacheKey {
            name: "host".to_owned(),
            record_type: RecordType::A,
        };
        let address: IpAddr = "192.0.2.1".parse().expect("valid");
        store(&cache, key.clone(), &[address], Duration::from_secs(60)).await;
        assert_eq!(cached(&cache, &key).await, Some(vec![address]));
    }
}
