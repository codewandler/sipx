//! What a running endpoint will tell you about itself.
//!
//! `tests/backpressure.rs` covers one counter reached one way: `Handle::shed`, placed by hand when
//! `T-19` found a request vanishing. These tests are about the destination those counters live in —
//! a snapshot read the same way, whose point is that the *next* counter has somewhere to go.
//!
//! Vectors X11 and X13 of `docs/specs/sip-transport.md` §11.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::RequestBuilder;
use sipx_sip::{HeaderName, Host, HostName, Method, Uri};
use sipx_transport::{Config, Handle, Incoming, Target, TransportKind, bind, new_branch};
use tokio::sync::mpsc::Receiver;

/// How long a test here waits for the loop to have counted something before concluding it never
/// will (`X-29`). A bound on failure, not a window to measure in: load can only lengthen the wait,
/// and nothing here asserts a count *after* a sleep.
const COUNTING_BOUND: Duration = Duration::from_secs(10);

/// Wait until something has happened, rather than sleeping and assuming it has (`X-29`, `X-40`).
async fn until(within: Duration, what: &str, mut condition: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + within;
    while !condition().await {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// An endpoint whose application queue holds exactly one message, so the second request sheds.
async fn saturated() -> (Handle, Receiver<Incoming>) {
    let mut config = Config::new("127.0.0.1:0".parse().expect("valid"));
    config.capacity = 1;
    bind(config).await.expect("binds")
}

async fn endpoint() -> (Handle, Receiver<Incoming>) {
    bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds")
}

fn to_uri() -> Uri {
    Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")))
}

fn request(sender: &Handle, method: &Method, call_id: &'static str) -> sipx_sip::Request {
    RequestBuilder::new(method.clone(), to_uri())
        .header(
            HeaderName::Via,
            Bytes::from(format!(
                "SIP/2.0/UDP {};rport;branch={}",
                sender.sent_by_for(TransportKind::Udp),
                new_branch()
            )),
        )
        .expect("via")
        .header(HeaderName::To, Bytes::from_static(b"<sip:callee.example>"))
        .expect("to")
        .header(
            HeaderName::From,
            Bytes::from_static(b"<sip:caller@example.net>;tag=abc"),
        )
        .expect("from")
        .header(HeaderName::CallId, Bytes::from_static(call_id.as_bytes()))
        .expect("call-id")
        .cseq(1, method)
        .expect("cseq")
        .max_forwards(70)
        .build()
}

/// A response that answers no transaction this endpoint started.
fn stray_response(target_via: &str, call_id: &str) -> Vec<u8> {
    format!(
        "SIP/2.0 200 OK\r\n\
         Via: SIP/2.0/UDP {target_via};branch=z9hG4bK-nothing-matches-this\r\n\
         To: <sip:callee.example>;tag=r\r\n\
         From: <sip:caller@example.net>;tag=abc\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: 1 OPTIONS\r\n\
         Content-Length: 0\r\n\r\n"
    )
    .into_bytes()
}

/// **The story's failing-first test** (vector X11).
///
/// Two losses that were counted in two unrelated ways — one in `ShedCounts`, one nowhere at all —
/// have to be readable from one place. The point is not either number: it is that a support case
/// asking "what did this endpoint throw away" has a single answer, and that the answer is reachable
/// synchronously while the loop is busy, which is the only time the question gets asked.
#[tokio::test]
async fn a_shed_request_and_an_unmatched_response_both_appear_in_the_counter_snapshot() {
    let (busy, mut incoming) = saturated().await;
    let (sender, _sender_incoming) = endpoint().await;
    let busy_addr = busy.local_addr();
    let busy_via = busy.sent_by_for(TransportKind::Udp);

    let before = busy.counters();
    assert!(
        !before.shed.any(),
        "a fresh endpoint has shed nothing: {before:?}"
    );
    assert_eq!(
        before.unmatched_responses, 0,
        "and has seen no unmatched response"
    );

    // Saturate: eight fresh transactions into a one-deep queue that is never read.
    for _ in 0..8u32 {
        let _ = sender
            .send_directly(
                request(&sender, &Method::Invite, "counters@sipx"),
                Target::udp(busy_addr),
            )
            .await;
    }

    // A response matching no client transaction, with nobody watching for it. It is right to drop
    // it; dropping it without counting is what this forbids.
    let stray = tokio::net::UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    stray
        .send_to(&stray_response(&busy_via, "stray-counter@sipx"), busy_addr)
        .await
        .expect("sends");

    until(
        COUNTING_BOUND,
        "a shed request and an unmatched response produced no counts",
        async || {
            let now = busy.counters();
            now.shed.any() && now.unmatched_responses > 0
        },
    )
    .await;

    let counters = busy.counters();
    assert!(
        counters.shed.any(),
        "requests were shed and the snapshot does not say so: {counters:?}"
    );
    assert!(
        counters.unmatched_responses > 0,
        "a response matched no transaction and the snapshot does not say so: {counters:?}"
    );

    // The snapshot agrees with the counter `T-19` placed by hand, rather than being a second,
    // separately-maintained tally of the same events.
    assert_eq!(
        counters.shed,
        busy.shed(),
        "the snapshot must embed ShedCounts, not duplicate it"
    );

    // Requests really did arrive over UDP, and the per-transport half of the snapshot saw them.
    // Without this the test would pass on an endpoint that counted losses and nothing else.
    let udp = counters.transport(TransportKind::Udp);
    assert!(
        udp.requests_in > 0,
        "eight requests arrived over UDP and the snapshot counted none: {udp:?}"
    );
    assert!(
        counters.transport(TransportKind::Tcp).requests_in == 0,
        "and nothing arrived over TCP, so that must stay zero"
    );

    assert!(
        incoming.try_recv().is_ok(),
        "the one queue slot should hold the first request, or nothing was ever delivered"
    );
}

/// An endpoint that lost nothing reports losing nothing. Without this every counter above could be
/// incremented unconditionally and the story's test would still pass.
#[tokio::test]
async fn an_undisturbed_endpoint_reports_no_losses() {
    let (calm, mut incoming) = endpoint().await;
    let (sender, _sender_incoming) = endpoint().await;

    let drain = tokio::spawn(async move {
        let mut seen = 0u32;
        while incoming.recv().await.is_some() {
            seen += 1;
            if seen == 4 {
                break;
            }
        }
        seen
    });

    for _ in 0..4u32 {
        let _ = sender
            .send_directly(
                request(&sender, &Method::Options, "calm-counters@sipx"),
                Target::udp(calm.local_addr()),
            )
            .await;
    }

    let seen = tokio::time::timeout(Duration::from_secs(2), drain)
        .await
        .expect("the application keeps up")
        .expect("the task finishes");
    assert_eq!(seen, 4);

    let counters = calm.counters();
    assert!(
        !counters.shed.any(),
        "delivered everything, yet reports shedding: {counters:?}"
    );
    assert_eq!(counters.unmatched_responses, 0);
    assert_eq!(
        counters.transport(TransportKind::Udp).parse_failures,
        0,
        "four well-formed requests are not parse failures"
    );
    assert_eq!(
        counters.transport(TransportKind::Udp).requests_in,
        4,
        "and all four were counted"
    );
}

/// Vector X13, and §12.2's second limit made executable: a malformed datagram is a parse failure
/// and is **not** counted as a request. The two assertions are one claim — if a malformed datagram
/// counted as both, an operator subtracting one from the other would get a plausible wrong answer.
#[tokio::test]
async fn a_malformed_datagram_counts_as_a_parse_failure_and_not_as_a_request() {
    let (endpoint, _incoming) = endpoint().await;
    let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await.expect("binds");

    // Not a SIP message and not STUN: STUN is diverted before the parser (§5) and would be
    // counted as neither.
    sender
        .send_to(b"NOT-A-SIP-MESSAGE\r\n\r\n", endpoint.local_addr())
        .await
        .expect("sends");

    until(COUNTING_BOUND, "a malformed datagram was not counted", async || {
        endpoint.counters().transport(TransportKind::Udp).parse_failures > 0
    })
    .await;

    let udp = endpoint.counters().transport(TransportKind::Udp);
    assert_eq!(udp.parse_failures, 1, "one malformed datagram, one failure");
    assert_eq!(
        udp.requests_in, 0,
        "a datagram that could not be parsed is not a request: which it would have been is \
         exactly what could not be determined"
    );
    assert_eq!(udp.responses_in, 0, "nor a response, for the same reason");
}

/// The snapshot is readable without asking the loop, which is §12's whole argument. `Handle::shed`
/// is synchronous and `Handle::outstanding` is not; this asserts the new snapshot took the first
/// shape, because a snapshot that needed the loop would be unavailable in the situation it
/// describes.
///
/// The assertion is a compile-time one — `counters()` is called in a non-`async` closure, which
/// only compiles if it neither awaits nor returns a future — plus the run-time check that it still
/// answers after the endpoint's loop has stopped.
#[tokio::test]
async fn the_snapshot_is_readable_without_the_loop() {
    let (endpoint, _incoming) = endpoint().await;

    let read_synchronously = || endpoint.counters();
    let _ = read_synchronously();

    endpoint.shutdown().await;
    until(
        COUNTING_BOUND,
        "the endpoint never stopped",
        async || endpoint.outstanding().await.is_err(),
    )
    .await;

    // The loop is gone. `outstanding()` now fails by construction; the snapshot must not, because
    // the counters are in shared atomics rather than behind the loop.
    let counters = read_synchronously();
    assert!(
        !counters.shed.any(),
        "a quiet endpoint that has stopped still answers, and answers zero: {counters:?}"
    );
}
