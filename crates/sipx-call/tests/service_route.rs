//! The registrar's outbound route set, obeyed (RFC 3608).
//!
//! `Path` (RFC 3327) fixed the direction *into* a UA behind proxies. This is the other one: the
//! registrar tells the UA which proxies its own requests must traverse, and a UA that ignores it
//! sends every call straight at the destination — arriving at a proxy that holds no state for the
//! registration the call belongs to, and being refused for a reason that looks nothing like the
//! cause.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::IpAddr;
use std::time::Duration;

use sipx_call::{DialOptions, dial};
use sipx_sip::{HeaderName, Host, HostName, Uri};
use sipx_transport::{Config, Handle, Incoming, Target, bind};
use tokio::sync::mpsc::Receiver;

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("valid")
}

async fn endpoint() -> (Handle, Receiver<Incoming>) {
    bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds")
}

fn to_uri() -> Uri {
    Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")))
}

/// The story's failing-first test.
#[tokio::test]
async fn a_preloaded_route_is_serialized_while_the_caller_targets_its_resolved_proxy() {
    let (proxy_endpoint, mut proxy_incoming) = endpoint().await;
    let (caller_endpoint, _caller_incoming) = endpoint().await;
    let resolved_proxy_target = proxy_endpoint.local_addr();

    // What a registrar handed back in the 2xx to REGISTER, in the order it listed them.
    let service_route = vec![
        "<sip:edge.example.com;lr>".to_owned(),
        "<sip:core.example.net;lr>".to_owned(),
    ];

    let seen = tokio::spawn(async move { proxy_incoming.recv().await.expect("an INVITE") });
    let _ = dial(
        &caller_endpoint,
        // The application resolved the outer Route hop. This transport target is deliberately
        // distinct from the callee.example Request-URI below.
        Target::udp(resolved_proxy_target),
        &to_uri(),
        &DialOptions::new("<sip:caller@example.net>", loopback())
            .with_timeout(Duration::from_millis(300))
            .with_service_route(service_route.clone()),
    )
    .await;

    let invite = seen.await.expect("the INVITE arrives").request;
    let routes: Vec<String> = invite
        .headers
        .get_all(&HeaderName::Route)
        .map(|header| String::from_utf8_lossy(&header.value()).into_owned())
        .collect();

    assert_eq!(
        routes, service_route,
        "the INVITE did not carry the registrar's service route as a pre-loaded Route set"
    );

    // Receipt at `proxy_endpoint` proves the caller-supplied Target selected the transport peer.
    // The Request-URI is untouched. RFC 3608 §5 requires every hop to carry `;lr`, which is
    // loose routing: the destination stays in the Request-URI and the proxies are named in
    // `Route`. A stack that moved the destination into the route set would be doing RFC 2543
    // strict routing, and the callee would answer a request addressed to a proxy.
    assert_eq!(
        String::from_utf8_lossy(&invite.uri.to_bytes()),
        "sip:callee.example",
        "the Request-URI was rewritten; the pre-loaded route set is loose, not strict"
    );
}

#[tokio::test]
async fn a_call_without_a_service_route_carries_no_route_header() {
    // The default has to be *no* Route. A stray empty route set is worse than none: it is a
    // header the far end must interpret, and some proxies answer 400 to a malformed one.
    let (peer_endpoint, mut peer_incoming) = endpoint().await;
    let (caller_endpoint, _caller_incoming) = endpoint().await;
    let peer_addr = peer_endpoint.local_addr();

    let seen = tokio::spawn(async move { peer_incoming.recv().await.expect("an INVITE") });
    let _ = dial(
        &caller_endpoint,
        Target::udp(peer_addr),
        &to_uri(),
        &DialOptions::new("<sip:caller@example.net>", loopback())
            .with_timeout(Duration::from_millis(300)),
    )
    .await;

    let invite = seen.await.expect("the INVITE arrives").request;
    assert_eq!(
        invite.headers.get_all(&HeaderName::Route).count(),
        0,
        "an INVITE with no service route should carry no Route header"
    );
}
