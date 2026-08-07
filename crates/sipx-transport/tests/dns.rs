//! RFC 3263 resolution through a real DNS client.
//!
//! Against a fixture nameserver on localhost, never the public internet: a test that depends
//! on someone else's zone file fails for reasons that have nothing to do with this code, and
//! passes for reasons that have nothing to do with it either.

#![cfg(feature = "dns")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use hickory_resolver::proto::op::{Message, MessageType, OpCode, ResponseCode};
use hickory_resolver::proto::rr::rdata::{A, SRV};
use hickory_resolver::proto::rr::{Name, RData, Record, RecordType};
use hickory_resolver::proto::serialize::binary::{BinDecodable, BinEncodable};
use sipx_transport::TransportKind;
use sipx_transport::dns::{
    Answer, DnsResolver, Prefetched, ResolutionError, ResolutionPolicy, resolve_uri_bounded,
};
use sipx_transport::resolve::{Resolver as _, SeededRng, resolve};
use tokio::net::UdpSocket;

/// A nameserver that answers from a fixed zone, so the test controls every record.
///
/// Returns the address and a count of the queries it has actually served — which is what makes
/// "served from cache" an assertion rather than a claim.
async fn fixture_server() -> (SocketAddr, Arc<std::sync::atomic::AtomicU32>) {
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let addr = socket.local_addr().expect("has an address");
    let queries = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let counter = Arc::clone(&queries);

    tokio::spawn(async move {
        let mut buf = vec![0u8; 2048];
        while let Ok((len, from)) = socket.recv_from(&mut buf).await {
            let Ok(request) = Message::from_bytes(&buf[..len]) else {
                continue;
            };
            let Some(query) = request.queries.first() else {
                continue;
            };
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

            let name = query.name().to_string();
            let mut response = Message::response(request.metadata.id, OpCode::Query);
            response.metadata.message_type = MessageType::Response;
            response.metadata.authoritative = true;
            response.metadata.recursion_available = true;
            response.queries.push(query.clone());

            match (query.query_type(), name.as_str()) {
                (RecordType::SRV, "_sip._udp.sipx.test.") => {
                    for (priority, port, target) in [
                        (10u16, 5060u16, "one.sipx.test."),
                        (20, 5062, "two.sipx.test."),
                    ] {
                        response.answers.push(Record::from_rdata(
                            Name::from_ascii(&name).expect("valid"),
                            60,
                            RData::SRV(SRV::new(
                                priority,
                                0,
                                port,
                                Name::from_ascii(target).expect("valid"),
                            )),
                        ));
                    }
                }
                (RecordType::A, "one.sipx.test.") => {
                    response.answers.push(Record::from_rdata(
                        Name::from_ascii(&name).expect("valid"),
                        60,
                        RData::A(A::new(192, 0, 2, 11)),
                    ));
                }
                (RecordType::A, "two.sipx.test.") => {
                    response.answers.push(Record::from_rdata(
                        Name::from_ascii(&name).expect("valid"),
                        60,
                        RData::A(A::new(192, 0, 2, 12)),
                    ));
                }
                _ => {
                    // A negative answer. A real one carries an SOA so the resolver knows how
                    // long to cache it; this one does not, which is what a synthesised NXDOMAIN
                    // also looks like — see `classify`.
                    response.metadata.response_code = ResponseCode::NXDomain;
                }
            }

            if let Ok(bytes) = response.to_bytes() {
                let _ = socket.send_to(&bytes, from).await;
            }
        }
    });

    (addr, queries)
}

/// The same fixture, answering after a delay.
///
/// The delay is what makes a stampede observable: without it the first lookup completes before
/// the second is issued, and every implementation looks single-flight.
async fn slow_fixture_server(delay: Duration) -> (SocketAddr, Arc<std::sync::atomic::AtomicU32>) {
    let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.expect("binds"));
    let addr = socket.local_addr().expect("has an address");
    let queries = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let counter = Arc::clone(&queries);

    tokio::spawn(async move {
        let mut buf = vec![0u8; 2048];
        while let Ok((len, from)) = socket.recv_from(&mut buf).await {
            let Ok(request) = Message::from_bytes(&buf[..len]) else {
                continue;
            };
            let Some(query) = request.queries.first().cloned() else {
                continue;
            };
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let socket = Arc::clone(&socket);
            tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                let name = query.name().to_string();
                let mut response = Message::response(request.metadata.id, OpCode::Query);
                response.metadata.message_type = MessageType::Response;
                response.metadata.authoritative = true;
                response.queries.push(query.clone());
                if query.query_type() == RecordType::A && name == "slow.sipx.test." {
                    response.answers.push(Record::from_rdata(
                        Name::from_ascii(&name).expect("valid"),
                        60,
                        RData::A(A::new(192, 0, 2, 33)),
                    ));
                } else {
                    response.metadata.response_code = ResponseCode::NXDomain;
                }
                if let Ok(bytes) = response.to_bytes() {
                    let _ = socket.send_to(&bytes, from).await;
                }
            });
        }
    });

    (addr, queries)
}

fn resolver_for(server: SocketAddr) -> Arc<DnsResolver> {
    Arc::new(DnsResolver::for_nameserver(server, Duration::from_secs(2)).expect("a resolver"))
}

/// The acceptance test for T-5: a URI naming a domain resolves at runtime, through a real
/// client, to the addresses the zone actually contains.
#[tokio::test]
async fn a_domain_uri_resolves_through_the_real_client() {
    let (server, _queries) = fixture_server().await;
    let resolver = resolver_for(server);
    let prefetched = Prefetched::for_domain(&resolver, "sipx.test").await;

    let uri = sipx_sip::Uri::sip(sipx_sip::Host::Name(
        sipx_sip::HostName::new("sipx.test").expect("valid"),
    ));
    let targets = resolve(&uri, &prefetched, &mut SeededRng::new(1));

    assert!(!targets.is_empty(), "the zone has SRV and A records");
    assert_eq!(
        targets[0].addr.to_string(),
        "192.0.2.11:5060",
        "priority 10 first: {targets:?}"
    );
    assert_eq!(targets[1].addr.to_string(), "192.0.2.12:5062");
}

#[tokio::test]
async fn srv_records_come_back_with_their_priorities_and_ports() {
    let (server, _queries) = fixture_server().await;
    let resolver = resolver_for(server);
    match resolver.srv("_sip._udp.sipx.test").await {
        Answer::Records(records) => {
            assert_eq!(records.len(), 2);
            assert_eq!(records[0].priority, 10);
            assert_eq!(records[0].port, 5060);
            assert_eq!(
                records[0].target, "one.sipx.test",
                "the trailing root dot must be stripped, or nothing matches it later"
            );
        }
        Answer::Unavailable => panic!("the fixture server answered"),
    }
}

/// A name the zone does not have is an answer, not a failure — but this fixture's NXDOMAIN
/// carries no SOA, so sipx treats it as unavailable and would retry. That is the deliberate
/// bias documented on `classify`: retrying a name that does not exist costs one lookup, while
/// caching a network blip as a routing decision costs every call to that domain.
#[tokio::test]
async fn a_name_the_zone_lacks_does_not_yield_records() {
    let (server, _queries) = fixture_server().await;
    let resolver = resolver_for(server);
    assert!(
        resolver
            .srv("_sip._udp.absent.test")
            .await
            .or_empty()
            .is_empty()
    );
}

/// Records are served from cache within their TTL rather than asked for again.
#[tokio::test]
async fn a_second_lookup_is_served_from_cache() {
    let (server, queries) = fixture_server().await;
    let resolver = resolver_for(server);

    let first = resolver.srv("_sip._udp.sipx.test").await.or_empty();
    assert_eq!(first.len(), 2);
    let after_first = queries.load(std::sync::atomic::Ordering::SeqCst);
    assert!(after_first > 0, "the first lookup reached the server");

    let second = resolver.srv("_sip._udp.sipx.test").await.or_empty();
    assert_eq!(second, first, "the same records");
    assert_eq!(
        queries.load(std::sync::atomic::Ordering::SeqCst),
        after_first,
        "the second lookup must not have reached the server at all"
    );
}

#[tokio::test]
async fn the_cache_capacity_is_one_total_across_record_types() {
    let (server, queries) = fixture_server().await;
    let resolver = DnsResolver::for_nameserver(server, Duration::from_secs(2))
        .expect("a resolver")
        .with_cache_capacity(1);

    assert_eq!(
        resolver.srv("_sip._udp.sipx.test").await.or_empty().len(),
        2
    );
    assert_eq!(
        resolver.addresses("one.sipx.test").await.or_empty().len(),
        1
    );
    assert_eq!(
        resolver.srv("_sip._udp.sipx.test").await.or_empty().len(),
        2
    );

    assert_eq!(
        queries.load(std::sync::atomic::Ordering::SeqCst),
        4,
        "A and AAAA share the one-entry budget with SRV, so the older SRV row is evicted"
    );
}

/// An entry past its TTL is asked for again rather than served stale. Holding a record past
/// its TTL points calls at a server that has moved.
#[tokio::test]
async fn an_expired_entry_is_asked_for_again() {
    let (server, queries) = fixture_server().await;
    // A short ceiling on the cache, so the 60-second TTL in the zone does not decide how long
    // this test takes.
    let resolver = DnsResolver::for_nameserver(server, Duration::from_secs(2))
        .expect("a resolver")
        .with_max_ttl(Duration::from_millis(200));

    resolver.srv("_sip._udp.sipx.test").await.or_empty();
    let after_first = queries.load(std::sync::atomic::Ordering::SeqCst);

    // Ordering a stimulus: both lookups are this test's, and the second has to be issued after
    // the 200 ms ceiling above has passed. A cache entry expiring is not an event — nothing is
    // evicted, nothing is signalled, the entry is simply not honoured next time — so there is
    // nothing to poll for, and load lengthening this window only puts the second lookup further
    // past the expiry (`X-44`).
    tokio::time::sleep(Duration::from_millis(300)).await;
    resolver.srv("_sip._udp.sipx.test").await.or_empty();
    assert!(
        queries.load(std::sync::atomic::Ordering::SeqCst) > after_first,
        "an expired entry must be re-asked, not served stale"
    );
}

/// Resolution must not block the caller's runtime for the whole lookup chain — and more to the
/// point, the endpoint loop never calls this at all: `Prefetched` does the awaiting up front
/// and hands the selection logic plain data.
#[tokio::test]
async fn prefetching_gathers_everything_the_selection_needs() {
    let (server, _queries) = fixture_server().await;
    let resolver = resolver_for(server);
    let prefetched = Prefetched::for_domain(&resolver, "sipx.test").await;

    // The trait it satisfies is synchronous: no await, no blocking, nothing to stall a loop.
    assert_eq!(prefetched.srv("_sip._udp.sipx.test").len(), 2);
    assert_eq!(prefetched.addresses("one.sipx.test").len(), 1);
    assert!(
        prefetched.naptr("sipx.test").is_empty(),
        "the zone has none"
    );
}

/// The `T-17` story's failing-first test.
///
/// A user agent resolves for its own one call, so a cache miss costs one query. A forwarding
/// element resolves for every call it forwards, and a burst to one domain arrives together — so
/// without single-flight, one cache miss becomes one query *per concurrent call*, every one of them
/// missing the cache because none of them has finished yet. That is a stampede aimed at whoever
/// runs the nameserver.
///
/// The coalescing is `hickory-resolver`'s, not sipx's — a single-flight layer written on top of it
/// was measured to change nothing and removed. This test is what makes that a checked fact rather
/// than an assumption: if the client is swapped or configured differently, it fails here.
#[tokio::test]
async fn concurrent_resolutions_make_one_query_per_address_family() {
    let (server, queries) = slow_fixture_server(Duration::from_millis(200)).await;
    let resolver = resolver_for(server);

    // Eight at once, all for the same name, all issued before any can complete.
    let mut tasks = Vec::new();
    for _ in 0..8u32 {
        let resolver = Arc::clone(&resolver);
        tasks.push(tokio::spawn(async move {
            resolver.addresses("slow.sipx.test").await
        }));
    }

    let mut answered = 0u32;
    for task in tasks {
        let answer = task.await.expect("the task finishes");
        if let Answer::Records(records) = answer
            && records.iter().any(|ip| ip.to_string() == "192.0.2.33")
        {
            answered += 1;
        }
    }

    assert_eq!(
        answered, 8,
        "every caller must get the answer, not only the leader"
    );
    assert_eq!(
        queries.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "eight concurrent lookups must make one shared A and one shared AAAA query"
    );
}

/// Single-flight must not turn into a cache: after the in-flight lookup completes and its TTL
/// expires, the next caller queries again.
#[tokio::test]
async fn single_flight_does_not_outlive_the_lookup_it_shares() {
    let (server, queries) = slow_fixture_server(Duration::from_millis(50)).await;
    let resolver = Arc::new(
        DnsResolver::for_nameserver(server, Duration::from_secs(2))
            .expect("a resolver")
            .with_max_ttl(Duration::from_millis(1)),
    );

    let _ = resolver.addresses("slow.sipx.test").await;
    // Ordering a stimulus, as in `an_expired_entry_is_asked_for_again`: the second lookup has to
    // be issued after the 1 ms ceiling above has passed, an expiry is not an event to wait for,
    // and load can only put the second lookup further past it (`X-44`).
    tokio::time::sleep(Duration::from_millis(20)).await;
    let _ = resolver.addresses("slow.sipx.test").await;

    assert_eq!(
        queries.load(std::sync::atomic::Ordering::SeqCst),
        4,
        "a lookup after the A and AAAA entries expired must ask for both again"
    );
}

/// A nameserver whose negative answers carry an SOA, as a real one's do (RFC 2308).
///
/// The plain fixture's NXDOMAIN has no SOA, which sipx reads as "could not ask" — so it cannot say
/// anything about caching a *genuine* negative. This one can.
async fn negative_fixture_server() -> (SocketAddr, Arc<std::sync::atomic::AtomicU32>) {
    use hickory_resolver::proto::rr::rdata::SOA;

    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let addr = socket.local_addr().expect("has an address");
    let queries = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let counter = Arc::clone(&queries);

    tokio::spawn(async move {
        let mut buf = vec![0u8; 2048];
        while let Ok((len, from)) = socket.recv_from(&mut buf).await {
            let Ok(request) = Message::from_bytes(&buf[..len]) else {
                continue;
            };
            let Some(query) = request.queries.first() else {
                continue;
            };
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

            let mut response = Message::response(request.metadata.id, OpCode::Query);
            response.metadata.message_type = MessageType::Response;
            response.metadata.authoritative = true;
            response.queries.push(query.clone());
            response.metadata.response_code = ResponseCode::NXDomain;
            // The SOA is what makes this a real negative answer rather than a synthesised one,
            // and its TTL is how long it may be remembered.
            response.authorities.push(Record::from_rdata(
                Name::from_ascii("sipx.test.").expect("valid"),
                300,
                RData::SOA(SOA::new(
                    Name::from_ascii("ns.sipx.test.").expect("valid"),
                    Name::from_ascii("admin.sipx.test.").expect("valid"),
                    1,
                    3600,
                    600,
                    86400,
                    300,
                )),
            ));
            if let Ok(bytes) = response.to_bytes() {
                let _ = socket.send_to(&bytes, from).await;
            }
        }
    });

    (addr, queries)
}

/// A nameserver that records the names it was asked about.
///
/// A count cannot answer "was this name prefetched" — a negative answer with no SOA is treated as
/// "could not ask" and deliberately not cached, so a second lookup of an absent name queries again
/// whether or not it was prefetched. The names themselves can answer it.
async fn logging_fixture_server() -> (SocketAddr, Arc<tokio::sync::Mutex<Vec<String>>>) {
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let addr = socket.local_addr().expect("has an address");
    let asked = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let log = Arc::clone(&asked);

    tokio::spawn(async move {
        let mut buf = vec![0u8; 2048];
        while let Ok((len, from)) = socket.recv_from(&mut buf).await {
            let Ok(request) = Message::from_bytes(&buf[..len]) else {
                continue;
            };
            let Some(query) = request.queries.first() else {
                continue;
            };
            log.lock()
                .await
                .push(format!("{} {}", query.query_type(), query.name()));

            let mut response = Message::response(request.metadata.id, OpCode::Query);
            response.metadata.message_type = MessageType::Response;
            response.metadata.authoritative = true;
            response.queries.push(query.clone());
            response.metadata.response_code = ResponseCode::NXDomain;
            if let Ok(bytes) = response.to_bytes() {
                let _ = socket.send_to(&bytes, from).await;
            }
        }
    });

    (addr, asked)
}

/// RFC 7118 §6's prefixes are prefetched too. A WebSocket destination that misses the prefetch is
/// not unreachable — it pays a serial round trip later, which is what prefetching exists to avoid.
#[tokio::test]
async fn the_websocket_srv_prefixes_are_prefetched() {
    let (server, asked) = logging_fixture_server().await;
    let resolver = resolver_for(server);
    let _ = Prefetched::for_domain(&resolver, "sipx.test").await;

    let asked = asked.lock().await.clone();
    for prefix in ["_sip._ws.", "_sips._wss."] {
        assert!(
            asked
                .iter()
                .any(|entry| entry == &format!("SRV {prefix}sipx.test.")),
            "{prefix}sipx.test was not prefetched; it would cost a serial round trip later. \
             Asked: {asked:?}"
        );
    }
    // And the three a phone needs are still there.
    for prefix in ["_sip._udp.", "_sip._tcp.", "_sips._tcp."] {
        assert!(
            asked
                .iter()
                .any(|entry| entry == &format!("SRV {prefix}sipx.test.")),
            "{prefix}sipx.test stopped being prefetched"
        );
    }
}

/// RFC 2308: a genuine negative answer is cacheable, and caching it is the difference between one
/// query per absent name and one per *call* to it. A forwarding element asks about the same absent
/// `_sips._tcp` record thousands of times a minute.
#[tokio::test]
async fn a_genuine_negative_answer_is_cached() {
    let (server, queries) = negative_fixture_server().await;
    let resolver = resolver_for(server);

    for _ in 0..4u32 {
        let answer = resolver.srv("_sips._tcp.sipx.test").await;
        assert!(
            matches!(answer, Answer::Records(ref records) if records.is_empty()),
            "an SOA-backed NXDOMAIN is a real negative, not an outage: {answer:?}"
        );
    }

    assert_eq!(
        queries.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "four lookups of a name the zone denies must ask once"
    );
}

/// And "could not ask" is *not* cached. Remembering a network blip as a routing decision keeps a
/// domain unreachable long after it has come back.
#[tokio::test]
async fn an_unavailable_answer_is_not_cached() {
    // Nothing listening: every lookup times out, and none of them is an answer to remember.
    let dead = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let addr = dead.local_addr().expect("has an address");
    drop(dead);
    let resolver = Arc::new(
        DnsResolver::for_nameserver(addr, Duration::from_millis(150)).expect("a resolver"),
    );

    for _ in 0..2u32 {
        assert!(
            matches!(
                resolver.srv("_sip._udp.sipx.test").await,
                Answer::Unavailable
            ),
            "an unanswered lookup is an outage, not a negative answer"
        );
    }
    // The assertion is that the second call still *asks* — proven by it still reporting
    // Unavailable rather than an empty cached set, which is what a cached negative would produce.
}

/// One await from a URI to a candidate list, for a caller that is not the endpoint loop.
#[tokio::test]
async fn a_uri_resolves_to_candidates_in_one_await() {
    let (server, _queries) = fixture_server().await;
    let resolver = resolver_for(server);
    let uri = sipx_sip::Uri::sip(sipx_sip::Host::Name(
        sipx_sip::HostName::new("sipx.test").expect("valid"),
    ));

    let targets = sipx_transport::dns::resolve_uri(&uri, &resolver, &mut SeededRng::new(1)).await;

    assert_eq!(
        targets
            .iter()
            .map(|target| target.addr.to_string())
            .collect::<Vec<_>>(),
        vec!["192.0.2.11:5060".to_owned(), "192.0.2.12:5062".to_owned()],
        "the same list the two-step form produces"
    );
}

#[tokio::test]
async fn a_literal_uri_performs_no_dns_question() {
    let (server, queries) = fixture_server().await;
    let resolver = resolver_for(server);
    let uri =
        sipx_sip::Uri::parse(bytes::Bytes::from_static(b"sip:alice@192.0.2.44:5090")).expect("URI");

    let targets = resolve_uri_bounded(
        &uri,
        &resolver,
        &mut SeededRng::new(1),
        None,
        ResolutionPolicy::default(),
    )
    .await
    .expect("literal target");

    assert_eq!(targets[0].addr.to_string(), "192.0.2.44:5090");
    assert_eq!(
        queries.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a literal is the complete route and must not touch DNS"
    );
}

#[tokio::test]
async fn a_named_explicit_port_asks_only_for_both_address_families() {
    let (server, queries) = fixture_server().await;
    let resolver = resolver_for(server);
    let uri = sipx_sip::Uri::parse(bytes::Bytes::from_static(b"sip:alice@one.sipx.test:5090"))
        .expect("URI");

    let targets = resolve_uri_bounded(
        &uri,
        &resolver,
        &mut SeededRng::new(1),
        Some(TransportKind::Udp),
        ResolutionPolicy::default(),
    )
    .await
    .expect("named explicit port");

    assert_eq!(targets[0].addr.to_string(), "192.0.2.11:5090");
    assert_eq!(
        queries.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "one A and one AAAA question, with no NAPTR or SRV"
    );
}

#[tokio::test]
async fn a_per_question_deadline_is_not_reported_as_an_empty_answer() {
    let (server, _queries) = slow_fixture_server(Duration::from_millis(200)).await;
    let resolver = resolver_for(server);
    let uri = sipx_sip::Uri::parse(bytes::Bytes::from_static(b"sip:alice@slow.sipx.test:5090"))
        .expect("URI");
    let policy = ResolutionPolicy {
        lookup_timeout: Duration::from_millis(10),
        resolution_timeout: Duration::from_secs(1),
        ..ResolutionPolicy::default()
    };

    let error = resolve_uri_bounded(
        &uri,
        &resolver,
        &mut SeededRng::new(1),
        Some(TransportKind::Udp),
        policy,
    )
    .await
    .expect_err("the delayed answer misses the question deadline");

    assert!(matches!(error, ResolutionError::LookupTimeout { .. }));
}

#[tokio::test]
async fn cancelling_resolution_joins_the_caller_owned_future() {
    let (server, queries) = slow_fixture_server(Duration::from_secs(1)).await;
    let resolver = resolver_for(server);
    let uri = sipx_sip::Uri::parse(bytes::Bytes::from_static(b"sip:alice@slow.sipx.test:5090"))
        .expect("URI");
    let task = tokio::spawn(async move {
        resolve_uri_bounded(
            &uri,
            &resolver,
            &mut SeededRng::new(1),
            Some(TransportKind::Udp),
            ResolutionPolicy::default(),
        )
        .await
    });

    tokio::time::timeout(Duration::from_secs(1), async {
        while queries.load(std::sync::atomic::Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("both family questions became active");
    task.abort();
    let joined = task.await.expect_err("the cancelled owner joins");
    assert!(joined.is_cancelled());
    assert_eq!(queries.load(std::sync::atomic::Ordering::SeqCst), 2);
}
