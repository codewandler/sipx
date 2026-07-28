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
use sipx_transport::dns::{Answer, DnsResolver, Prefetched};
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
