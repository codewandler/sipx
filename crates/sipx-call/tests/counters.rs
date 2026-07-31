//! The signalling path's numbers, read as one thing beside the capture that explains them.
//!
//! `docs/specs/sip-transport.md` §12.1 says every discard in the signalling path is counted, and
//! §12.3 says which crates that path is and why the two sets of atomics behind it are still two.
//! This is the end-to-end demonstration the milestone's third clause asks for: a discard the
//! *dialog* layer owns, counted, next to a capture of the request that caused it.
//!
//! The precedent is `a_datagram_that_does_not_parse_is_still_captured`
//! (`crates/sipx-transport/tests/capture.rs`), which does exactly this one layer down for a parse
//! failure. What that test cannot show is the thing `X-51` found missing: it reads one crate's
//! snapshot, and an operator holding the capture has the other crate's loss nowhere to look for.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sipx_call::{Calls, Dispatcher, SignallingCounts};
use sipx_sip::{HeaderName, Host, HostName, Method, Request, Uri};
use sipx_transport::{CaptureConfig, Config, Handle, Incoming, Target, bind};
use tokio::sync::mpsc::Receiver;

/// A bound on failure, not a window to measure in (`X-29`). The capture writer is on a thread of
/// its own and the dispatcher is a task of its own, so both finish when they finish; nothing here
/// sleeps and then asserts.
const WRITING_BOUND: Duration = Duration::from_secs(10);

/// Poll for a condition until it holds, or fail at the deadline.
///
/// The loop waits for the thing; the interval below only decides how often it asks.
async fn until(within: Duration, what: &str, mut condition: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + within;
    while !condition().await {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn capture_path(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "sipx-call-counters-{}-{name}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    directory.join("signalling.pcapng")
}

/// An endpoint recording everything it exchanges to `path` (§13).
async fn recording(path: &Path) -> (Handle, Receiver<Incoming>) {
    let mut config = Config::new("127.0.0.1:0".parse().expect("valid"));
    config.capture = Some(CaptureConfig::new(path));
    bind(config).await.expect("binds with a capture")
}

async fn plain() -> (Handle, Receiver<Incoming>) {
    bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds")
}

fn callee_uri() -> Uri {
    Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")))
}

/// An ACK naming a dialog that does not exist, built by hand so the test knows its exact bytes.
fn stray_ack(peer: &Handle, call_id: &str) -> Request {
    let via = bytes::Bytes::from(format!(
        "SIP/2.0/UDP {};rport;branch={}",
        peer.sent_by_for(sipx_transport::TransportKind::Udp),
        sipx_transport::new_branch()
    ));
    sipx_sip::build::RequestBuilder::new(Method::Ack, callee_uri())
        .header(HeaderName::Via, via)
        .expect("via")
        .header(
            HeaderName::To,
            bytes::Bytes::from_static(b"<sip:callee.example>;tag=theirs"),
        )
        .expect("to")
        .header(
            HeaderName::From,
            bytes::Bytes::from_static(b"<sip:peer@example.net>;tag=ours"),
        )
        .expect("from")
        .header(HeaderName::CallId, bytes::Bytes::from(call_id.to_owned()))
        .expect("call-id")
        .cseq(1, &Method::Ack)
        .expect("cseq")
        .max_forwards(70)
        .build()
}

/// A dispatcher pumped by a task of its own, as a host uses one.
fn pump(endpoint: &Handle, incoming: Receiver<Incoming>) -> Calls {
    let mut dispatcher = Dispatcher::new(endpoint.clone(), incoming);
    let calls = dispatcher.calls();
    tokio::spawn(async move { while dispatcher.next().await.is_some() {} });
    calls
}

async fn tell(peer: &Handle, callee: SocketAddr, request: Request) {
    peer.send(request, Target::udp(callee))
        .await
        .expect("sends");
}

/// **M12's third clause, end to end.**
///
/// An ACK for a call this endpoint does not have is the clearest discard the dialog layer owns:
/// SIP has no response to an ACK (RFC 3261 §17.1.1.3), so it cannot be refused and a counter is
/// the whole of what can be done — and it is the one that leaks calls, which is why
/// `DispatchCounts::acks` is counted apart from every other refusal.
///
/// The assertion that matters is that **both halves are readable from one place**. Before `X-54`
/// the transport's snapshot and the dispatcher's were two structs in two crates that nothing but
/// each crate's own tests ever read, so "counted **and exportable next to** a capture" was two
/// features that existed separately.
#[tokio::test]
async fn a_discard_in_the_dialog_layer_is_counted_next_to_the_capture_of_the_request_that_caused_it()
{
    const CALL_ID: &str = "stray-ack-beside-its-capture@sipx";

    let path = capture_path("stray-ack");
    let (callee, callee_incoming) = recording(&path).await;
    let callee_addr = callee.local_addr();
    let calls = pump(&callee, callee_incoming);

    let (peer, _peer_incoming) = plain().await;
    tell(&peer, callee_addr, stray_ack(&peer, CALL_ID)).await;

    // One snapshot, asked once, covering both crates' loss.
    until(
        WRITING_BOUND,
        "the stray ACK was not counted in the joined signalling snapshot",
        async || {
            SignallingCounts::with_dispatcher(&callee, &calls)
                .dispatch
                .is_some_and(|dispatch| dispatch.acks > 0)
        },
    )
    .await;

    let counts = SignallingCounts::with_dispatcher(&callee, &calls);
    let dispatch = counts.dispatch.expect("a dispatcher is running");
    assert_eq!(dispatch.acks, 1, "counted apart: {dispatch:?}");
    assert_eq!(dispatch.total(), 1, "and nothing else moved: {dispatch:?}");
    assert!(
        counts.any_loss(),
        "a joined snapshot that has lost something must say so: {counts:?}"
    );

    // And the request that caused it is in the capture, so the number has something to point at.
    until(
        WRITING_BOUND,
        "the ACK that caused the discard never reached the capture",
        async || {
            std::fs::read(&path).is_ok_and(|bytes| {
                bytes
                    .windows(CALL_ID.len())
                    .any(|window| window == CALL_ID.as_bytes())
            })
        },
    )
    .await;

    let _ = std::fs::remove_dir_all(path.parent().expect("a scratch directory"));
}

/// The transport's half is in the same snapshot, and reads the same as `Handle::counters`.
///
/// The join must not become a second tally: two counts of one event eventually disagree, and then
/// neither can be trusted (§12.3).
#[tokio::test]
async fn the_joined_snapshot_reports_the_transport_half_unaltered() {
    let (endpoint, _incoming) = plain().await;

    let counts = SignallingCounts::of(&endpoint);
    assert_eq!(
        counts.transport,
        endpoint.counters(),
        "the joined snapshot must embed the transport's own numbers, not recount them"
    );
    assert!(
        counts.dispatch.is_none(),
        "no dispatcher is running, and `None` is not the same claim as zero"
    );
    assert!(!counts.any_loss(), "a fresh endpoint has lost nothing");
}
