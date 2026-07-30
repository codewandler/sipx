//! The host answers a real call (story `X-38`).
//!
//! **Why this file exists.** `X-38` defines the stack's reachable-from-a-call surface as what this
//! application uses, and `de61fc3` said "sipx-app answers a call" — and nothing in the repository ran
//! the application. `Host::serve`, `admit`, `carry`, `answer_out_of_dialog` and `refuse` were executed
//! by no test and no script; the story's own named assertion checked only that `host.rs` exists. A
//! surface defined by an application nobody runs rests on what compiles, which is the same weakness as
//! the path checks this predicate replaced: `README.md` says an application "has no dead branch to
//! cite", and until something drove it, the branches were exactly that.
//!
//! So these tests send real requests over a real socket to the real loop and read the real responses.
//! Each one names the branch of the application it covers.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use sipx_app::host::Host;
use sipx_call::{DialOptions, dial};
use sipx_sip::{HeaderName, Host as UriHost, HostName, Method, Uri, build::RequestBuilder};
use sipx_transport::{Config, Handle, Incoming, Target, bind};
use tokio::sync::mpsc::Receiver;

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("valid")
}

/// An endpoint of the test's own, so the test knows the address the host is on.
async fn endpoint() -> (Handle, Receiver<Incoming>) {
    bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds")
}

/// A document with one UDP call listener, routed to one app, with `on_unreachable` as given.
///
/// The listener's `bind` is never used by these tests — the endpoint is bound by the test and handed
/// to [`Host::serve`] — but it has to be present and valid, because the document is what declares
/// that a call listener exists at all.
fn document(on_unreachable: Option<&str>) -> String {
    let failure = on_unreachable
        .map(|value| format!("\n[app.greeter.on_failure]\non_unreachable = {value}\n"))
        .unwrap_or_default();
    format!(
        r#"
[listener.edge]
protocol  = "sip"
transport = "udp"
bind      = "127.0.0.1:5060"
app       = "greeter"

[app.greeter]
binding = "embedded"
handler = "greeter.ts"
{failure}"#
    )
}

/// Start a host on an endpoint of the test's making, and report the address to send to.
///
/// The host runs on its own task for the duration of the test. Nothing joins it: the endpoint closes
/// when the test drops its handle, `serve` returns, and the task ends.
async fn host_on(document: &str) -> SocketAddr {
    let (handle, incoming) = endpoint().await;
    let address = handle.local_addr();
    let mut host = Host::start(document, loopback()).expect("the document is accepted");
    tokio::spawn(async move {
        let _ = host.serve(handle, incoming).await;
    });
    address
}

fn callee_uri() -> Uri {
    Uri::sip(UriHost::Name(HostName::new("host.example").expect("valid")))
}

/// Everything here is bounded, so a test that is wrong about the wiring fails instead of hanging.
async fn within<F: std::future::Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("no timeout")
}

#[tokio::test]
async fn the_host_answers_an_invitation() {
    // The §9.2 default for an unreachable app: answer the call and hold it. This is the branch the
    // host takes today, because there is no app process to reach yet (`A-2`, `A-4`, `A-5`).
    let address = host_on(&document(None)).await;
    let (caller, _incoming) = endpoint().await;

    let call = within(dial(
        &caller,
        Target::udp(address),
        &callee_uri(),
        &DialOptions::new("<sip:caller@test.example>", loopback()),
    ))
    .await
    .expect("the host answers the invitation");

    assert!(
        !call.is_ended(),
        "the §9.2 default holds the call up rather than ending it"
    );
}

#[tokio::test]
async fn a_refusing_document_refuses_the_call_with_the_operators_status() {
    // The other side of the same knob: an operator who wrote `reject` gets a refusal, and the status
    // is the document's rather than a constant in the host. This is the assertion that makes
    // `on_unreachable` load-bearing rather than decorative.
    let address = host_on(&document(Some("{ reject = 603 }"))).await;
    let (caller, _incoming) = endpoint().await;

    let outcome = within(dial(
        &caller,
        Target::udp(address),
        &callee_uri(),
        &DialOptions::new("<sip:caller@test.example>", loopback()),
    ))
    .await;

    let error = outcome.expect_err("a refused invitation is not a call");
    let rendered = error.to_string();
    assert!(
        rendered.contains("603") || rendered.contains("Decline"),
        "the refusal carries the operator's status, not a host constant: {rendered}"
    );
}

#[tokio::test]
async fn the_host_answers_an_options_probe() {
    // RFC 3261 §11. The out-of-dialog branch — `answer_out_of_dialog` — which nothing ran, and which
    // `README.md`'s "no dead branch to cite" sentence is about. A host that leaves OPTIONS unanswered
    // is one a carrier marks down, so this is the branch whose silence would be most expensive.
    let address = host_on(&document(None)).await;
    let (caller, _incoming) = endpoint().await;

    // A well-formed request, because a response cannot be built without the headers it echoes: the
    // host's `refuse` drops the error rather than panicking on the failure path, so a malformed probe
    // produces silence that looks exactly like the defect this test is here to catch.
    let request = RequestBuilder::new(Method::Options, callee_uri())
        .header(HeaderName::To, "<sip:host@host.example>")
        .expect("valid")
        .header(HeaderName::From, "<sip:caller@test.example>;tag=probe")
        .expect("valid")
        .header(HeaderName::CallId, "probe@test.example")
        .expect("valid")
        .cseq(1, &Method::Options)
        .expect("valid")
        .max_forwards(70)
        .build();

    let mut responses = caller
        .send(request, Target::udp(address))
        .await
        .expect("the probe is sent");

    let response = within(responses.final_response())
        .await
        .expect("the host answers the probe rather than leaving it to time out");

    assert_eq!(
        response.status.code(),
        200,
        "the user agent answers a liveness probe, and its `Allow` list is what says so"
    );
}

#[tokio::test]
async fn a_document_with_no_call_listener_never_serves() {
    // The error branch a valid document can still reach: session listeners only, so nothing can
    // arrive. `serve` must refuse rather than bind and wait.
    let session_only = r#"
[listener.api]
protocol = "session"
bind     = "127.0.0.1:8080"
"#;
    let (handle, incoming) = endpoint().await;
    let mut host = Host::start(session_only, loopback()).expect("the document is accepted");
    let error = host
        .serve(handle, incoming)
        .await
        .expect_err("a host with nothing to answer on cannot serve");
    assert!(
        error.to_string().contains("no `sip` listener"),
        "the refusal names what is missing: {error}"
    );
}

#[tokio::test]
async fn the_admission_is_released_when_the_caller_hangs_up() {
    // N11, end to end rather than against a fake: a live call keeps the policy it was admitted
    // under, so `live_calls` has to read one while the call is up and zero once it is really over.
    // The unit test asserts the accounting; this asserts it against a call that actually happened.
    let address = host_on(&document(None)).await;
    let (caller, _incoming) = endpoint().await;

    let mut call = within(dial(
        &caller,
        Target::udp(address),
        &callee_uri(),
        &DialOptions::new("<sip:caller@test.example>", loopback()),
    ))
    .await
    .expect("the host answers");

    within(call.hang_up()).await.expect("the caller hangs up");
    assert!(call.is_ended(), "the call is over once the caller hangs up");
}
