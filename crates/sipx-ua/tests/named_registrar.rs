//! A library consumer reaching a registrar it can only name.
//!
//! This is the parity test: everything here is done through `sipx-transport` and `sipx-ua`, with
//! no CLI process anywhere, because the predicate that matters is an application nobody here wrote
//! meeting this stack through the library. If bounded RFC 3263 resolution is reachable only from
//! `sipx-cli`, this file cannot be written at all — which is the point of writing it.
//!
//! The zone is a fixture nameserver on localhost, never the host's own resolver: a test that
//! depends on somebody else's zone file fails for reasons that have nothing to do with this code,
//! and passes for reasons that have nothing to do with it either.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::ResponseBuilder;
use sipx_sip::{HeaderName, StatusCode, Uri};
use sipx_transport::destination::{Kind, Resolver};
use sipx_transport::dns::DnsResolver;
use sipx_transport::{CleartextTransports, Config as TransportConfig, Handle, TransportKind, bind};
use sipx_ua::{Config, UserAgent};
use tokio::net::UdpSocket;

/// The one name the fixture zone has an address for.
const RESOLVABLE: &str = "registrar.test";
/// A name the fixture zone answers about and has no address for.
const ABSENT: &str = "absent.test";
/// A name the fixture zone has three addresses for, all of them loopback, so a serial pass over
/// its candidates never puts a packet on a network.
const SPREAD: &str = "spread.test";
/// The last octet of each `SPREAD` address. `127.0.0.0/8` is this machine throughout.
const SPREAD_HOSTS: [u8; 3] = [1, 2, 3];

/// RFC 1035 §3.2.2 `A`.
const A: u16 = 1;

/// The question a datagram asks: its name in dotted form, its type, and where it ends.
///
/// Deliberately not a DNS library. A question is a length-prefixed name, a type and a class, and
/// hand-decoding those thirty bytes keeps the fixture free of the client under test — a zone
/// served through the same crate the resolver asks with would agree with itself about a
/// misencoded record.
fn question(datagram: &[u8]) -> Option<(String, u16, usize)> {
    let mut at = 12;
    let mut name = String::new();
    loop {
        let length = *datagram.get(at)? as usize;
        at += 1;
        if length == 0 {
            break;
        }
        // Compression pointers are legal in an answer and not in a question.
        if length >= 0xc0 {
            return None;
        }
        name.push_str(&String::from_utf8_lossy(datagram.get(at..at + length)?));
        name.push('.');
        at += length;
    }
    let kind = u16::from_be_bytes([*datagram.get(at)?, *datagram.get(at + 1)?]);
    Some((name, kind, at + 4))
}

/// A nameserver that knows exactly one address record.
///
/// Every other question is answered, and answered empty: that is a server saying "nothing here"
/// without the SOA that would make it a cacheable negative, which is how the adapter is told a
/// name could not be resolved rather than does not exist. Silence is a different fixture below.
async fn zone() -> SocketAddr {
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let address = socket.local_addr().expect("has an address");
    tokio::spawn(async move {
        let mut buffer = vec![0u8; 1024];
        while let Ok((length, from)) = socket.recv_from(&mut buffer).await {
            let Some((name, kind, end)) = question(&buffer[..length]) else {
                continue;
            };
            // A pointer to the question's name, then IN A 127.0.0.<host> with a one-second TTL.
            let record = |host: u8| [0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 1, 0, 4, 127, 0, 0, host];
            let (count, answer): (u16, Vec<u8>) = if kind != A {
                (0, Vec::new())
            } else if name == format!("{RESOLVABLE}.") {
                (1, record(1).to_vec())
            } else if name == format!("{SPREAD}.") {
                (
                    u16::try_from(SPREAD_HOSTS.len()).expect("three records"),
                    SPREAD_HOSTS.iter().flat_map(|host| record(*host)).collect(),
                )
            } else {
                (0, Vec::new())
            };
            let mut response = Vec::with_capacity(end + answer.len());
            response.extend_from_slice(&buffer[0..2]);
            // Response, recursion desired and available, no error.
            response.extend_from_slice(&[0x81, 0x80]);
            response.extend_from_slice(&[0, 1]);
            response.extend_from_slice(&count.to_be_bytes());
            // No authority and no additional section: an empty answer with no SOA is the
            // "could not resolve" the adapter distinguishes from a real absence.
            response.extend_from_slice(&[0, 0, 0, 0]);
            response.extend_from_slice(&buffer[12..end]);
            response.extend_from_slice(&answer);
            let _ = socket.send_to(&response, from).await;
        }
    });
    address
}

/// A resolver asking one nameserver, under one stated budget.
fn resolver(nameserver: SocketAddr, budget: Duration) -> Resolver {
    let client = DnsResolver::for_nameserver(nameserver, Duration::from_secs(2)).expect("client");
    Resolver::over(Arc::new(client), Some(budget))
}

/// A registrar that grants every lease it is asked for, over UDP.
async fn registrar() -> Handle {
    let (handle, mut incoming) = bind(TransportConfig::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");
    let responder = handle.clone();
    tokio::spawn(async move {
        while let Some(request) = incoming.recv().await {
            let contact = request
                .request
                .headers
                .value(&HeaderName::Contact)
                .map(|value| String::from_utf8_lossy(&value).into_owned())
                .expect("a REGISTER carries the contact it registers");
            let response = ResponseBuilder::to_request(
                &request.request,
                StatusCode::new(200).expect("valid"),
                "OK",
            )
            .expect("builds")
            .header(
                HeaderName::Contact,
                Bytes::from(format!("{contact};expires=600")),
            )
            .expect("valid")
            .build();
            let _ = responder.respond(&request.key, response).await;
        }
    });
    handle
}

/// An endpoint for the agent side of a test.
async fn endpoint() -> Handle {
    let (handle, _incoming) = bind(TransportConfig::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");
    handle
}

fn uri(text: String) -> Uri {
    Uri::parse(Bytes::from(text)).expect("a SIP URI")
}

/// The whole story in one pass: a name, a lookup nobody had to write, and a granted lease.
#[tokio::test]
async fn a_named_registrar_is_resolved_and_registered() {
    let nameserver = zone().await;
    let registrar = registrar().await;
    let port = registrar.local_addr().port();
    let agent_endpoint = endpoint().await;
    let contact = format!("<sip:alice@{}>", agent_endpoint.local_addr());

    let config = Config::resolved(
        format!("<sip:alice@{RESOLVABLE}>"),
        contact,
        uri(format!("sip:{RESOLVABLE}:{port}")),
        &resolver(nameserver, Duration::from_secs(4)),
    )
    .await
    .expect("the registrar's name resolves");

    assert_eq!(
        config.target.addr,
        format!("127.0.0.1:{port}")
            .parse::<SocketAddr>()
            .expect("valid"),
        "the address came from the zone rather than from the caller"
    );

    let lease = UserAgent::new(agent_endpoint, config)
        .register()
        .await
        .expect("the registrar granted a lease");
    assert_eq!(lease.granted, Duration::from_secs(600));
}

/// The identity rule, which is the one an application would otherwise have to know it had.
///
/// A `sips:` URI resolves to an address, and the certificate presented at that address is checked
/// against the *name* — not against whatever the zone happens to call the host it points at.
#[tokio::test]
async fn a_secure_registrar_keeps_its_name_as_the_verification_identity() {
    let nameserver = zone().await;
    let config = Config::resolved(
        format!("<sip:alice@{RESOLVABLE}>"),
        "<sip:alice@127.0.0.1:5060>",
        uri(format!("sips:{RESOLVABLE}:5061")),
        &resolver(nameserver, Duration::from_secs(4)),
    )
    .await
    .expect("the registrar's name resolves");

    assert_eq!(
        config.target.transport,
        TransportKind::Tls,
        "a sips: URI never falls back to cleartext"
    );
    assert_eq!(
        config.target.verify_as.as_deref(),
        Some(RESOLVABLE),
        "the name asked for, not the address resolved to, is the verification identity"
    );
}

/// Three failures an application has to tell apart, because each has a different owner: the zone
/// is wrong, the resolver is unreachable, or the address is right and nothing is listening.
#[tokio::test]
async fn resolution_failure_resolution_timeout_and_connection_failure_are_distinct() {
    let nameserver = zone().await;

    // The zone answers, and has nothing to say about this name.
    let failed = Config::resolved(
        format!("<sip:alice@{ABSENT}>"),
        "<sip:alice@127.0.0.1:5060>",
        uri(format!("sip:{ABSENT}:5060")),
        &resolver(nameserver, Duration::from_secs(4)),
    )
    .await
    .expect_err("no address for the name");
    assert_eq!(
        kind(&failed),
        Some(Kind::Resolution),
        "a zone with no answer is a resolution failure: {failed}"
    );

    // A nameserver that is reachable and never answers. The budget is what ends it.
    let silent = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let quiet = silent.local_addr().expect("has an address");
    let timed_out = Config::resolved(
        format!("<sip:alice@{RESOLVABLE}>"),
        "<sip:alice@127.0.0.1:5060>",
        uri(format!("sip:{RESOLVABLE}:5060")),
        &resolver(quiet, Duration::from_millis(200)),
    )
    .await
    .expect_err("nothing answered the lookup");
    assert_eq!(
        kind(&timed_out),
        Some(Kind::Timeout),
        "a nameserver that never answers is a deadline, not an absence: {timed_out}"
    );

    // The name resolves. What is listening on that port is UDP, and the URI asks for TCP.
    let (udp_only, _incoming) = bind(TransportConfig {
        cleartext: CleartextTransports::Udp,
        ..TransportConfig::new("127.0.0.1:0".parse().expect("valid"))
    })
    .await
    .expect("binds");
    let port = udp_only.local_addr().port();
    let agent_endpoint = endpoint().await;
    let contact = format!("<sip:alice@{}>", agent_endpoint.local_addr());
    let config = Config::resolved(
        format!("<sip:alice@{RESOLVABLE}>"),
        contact,
        uri(format!("sip:{RESOLVABLE}:{port};transport=tcp")),
        &resolver(nameserver, Duration::from_secs(4)),
    )
    .await
    .expect("the registrar's name resolves");
    assert_eq!(config.target.transport, TransportKind::Tcp);

    let refused = UserAgent::new(agent_endpoint, config)
        .register()
        .await
        .expect_err("nothing accepts TCP on that port");
    assert!(
        matches!(refused, sipx_ua::Error::Transport(_)),
        "the name resolved; the connection is what failed: {refused}"
    );
    assert_eq!(
        kind(&refused),
        None,
        "a connection failure is not a resolution failure"
    );
}

/// `T-41`: the count reaches a library consumer, not only the diagnostic phone.
///
/// `library-parity` is the epic, and a capability that stops at the CLI is exactly what it exists
/// to close. An application that can only see the last transport error cannot tell one dead host
/// behind a name from every address behind it being unreachable — which is the same sentence the
/// operator-facing half of this story is about, one layer down.
///
/// The counts are attempted and resolved rather than attempted alone, because `P-26` makes a
/// caller's budget the ceiling over the whole serial pass: `attempted` is attempted-so-far, and it
/// takes the second number to say whether the pass ran out of candidates or out of time.
#[tokio::test]
async fn a_connection_failure_reports_the_candidates_it_attempted() {
    let nameserver = zone().await;
    let resolver = resolver(nameserver, Duration::from_secs(4));

    // A port on this machine that nothing accepts on: reserved to learn the number, then released.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").expect("reserves a loopback port");
    let refused = closed.local_addr().expect("reserved address").port();
    drop(closed);

    let attempted = |host: &'static str| {
        let resolver = &resolver;
        async move {
            let registrar = uri(format!("sip:{host}:{refused};transport=tcp"));
            let candidates = resolver
                .resolve(&registrar, None, None)
                .await
                .expect("the name resolves");
            let agent_endpoint = endpoint().await;
            let contact = format!("<sip:alice@{}>", agent_endpoint.local_addr());
            let config = Config::new(
                format!("<sip:alice@{host}>"),
                contact,
                registrar,
                candidates[0].clone(),
            );
            let failure = UserAgent::register_candidates(agent_endpoint, config, &candidates, None)
                .await
                .expect_err("nothing accepts TCP on that port");
            (candidates.len(), failure)
        }
    };

    // One address behind the name. One host refused; there was nowhere else to go.
    let (listed, one) = attempted(RESOLVABLE).await;
    assert_eq!(listed, 1);
    let sipx_ua::Error::ConnectionFailed { attempts, .. } = &one else {
        panic!("a refused connection over a resolved name is a connection failure: {one}");
    };
    assert_eq!(attempts.attempted(), 1, "{one}");
    assert_eq!(attempts.resolved(), 1, "{one}");
    assert!(attempts.exhausted(), "{one}");
    let single = attempts.attempted();

    // Three addresses behind the name. The pass walked the ordered list to its end.
    let (listed, every) = attempted(SPREAD).await;
    assert_eq!(listed, 3, "the zone offers three addresses");
    let sipx_ua::Error::ConnectionFailed { attempts, .. } = &every else {
        panic!("a refused connection over a resolved name is a connection failure: {every}");
    };
    assert_eq!(attempts.attempted(), 3, "{every}");
    assert_eq!(attempts.resolved(), 3, "{every}");
    assert!(
        attempts.exhausted(),
        "nothing cut this pass short, so it is exhausted rather than abandoned: {every}"
    );
    assert_ne!(
        single,
        attempts.attempted(),
        "the two failures must not report the same count, or the field says nothing"
    );

    // The message an application prints without matching carries it too, and still names the
    // transport cause `T-39` published rather than a resolution that succeeded.
    let rendered = every.to_string();
    assert!(
        rendered.contains("transport") && !rendered.contains("resolution"),
        "{rendered}"
    );
    assert!(rendered.contains("3 of 3"), "{rendered}");
}

/// The resolution classification behind a user agent error, if it has one.
fn kind(error: &sipx_ua::Error) -> Option<Kind> {
    match error {
        sipx_ua::Error::Resolution(resolution) => Some(resolution.kind()),
        _ => None,
    }
}
