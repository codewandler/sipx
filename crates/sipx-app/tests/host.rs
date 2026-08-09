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
use sipx_call::{DialOptions, Dispatched, Dispatcher, dial};
use sipx_sip::{HeaderName, Host as UriHost, HostName, Method, Uri, build::RequestBuilder};
use sipx_transport::{Config, Handle, Incoming, Target, bind};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc::Receiver;
use tokio::sync::oneshot;

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

async fn webhook_host_on(document: &str) -> SocketAddr {
    let (handle, incoming) = endpoint().await;
    let address = handle.local_addr();
    let mut host = Host::start_with_secrets(document, loopback(), |name| {
        (name == "hook").then(|| b"test-secret".to_vec())
    })
    .expect("the webhook document is accepted");
    tokio::spawn(async move {
        let _ = host.serve(handle, incoming).await;
    });
    address
}

fn webhook_document(url: &str, knob: &str, status: u16, timeout_ms: u32) -> String {
    format!(
        r#"
[listener.edge]
protocol  = "sip"
transport = "udp"
bind      = "127.0.0.1:5060"
app       = "greeter"

[app.greeter]
binding = "webhook"
url = "{url}"
signing_secrets = ["hook"]

[app.greeter.on_failure]
timeout_ms = {timeout_ms}
{knob} = {{ reject = {status} }}
"#
    )
}

async fn status_peer(statuses: Vec<u16>) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let url = format!("http://{}/hook", listener.local_addr().expect("address"));
    let task = tokio::spawn(async move {
        for status in statuses {
            let (mut socket, _) = listener.accept().await.expect("accepts");
            let mut request = [0_u8; 4096];
            let _ = socket.read(&mut request).await.expect("reads");
            let response =
                format!("HTTP/1.1 {status} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            socket.write_all(response.as_bytes()).await.expect("writes");
        }
    });
    (url, task)
}

async fn refused_by_webhook(document: &str, expected: u16) {
    let address = webhook_host_on(document).await;
    let (caller, _incoming) = endpoint().await;
    let error = Box::pin(within(dial(
        &caller,
        Target::udp(address),
        &callee_uri(),
        &DialOptions::new("<sip:caller@test.example>", loopback()),
    )))
    .await
    .expect_err("the declared failure refuses the invitation");
    assert!(
        error.to_string().contains(&expected.to_string()),
        "the refusal carries declared status {expected}: {error}"
    );
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

    let call = Box::pin(within(dial(
        &caller,
        Target::udp(address),
        &callee_uri(),
        &DialOptions::new("<sip:caller@test.example>", loopback()),
    )))
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

    let outcome = Box::pin(within(dial(
        &caller,
        Target::udp(address),
        &callee_uri(),
        &DialOptions::new("<sip:caller@test.example>", loopback()),
    )))
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

    let mut call = Box::pin(within(dial(
        &caller,
        Target::udp(address),
        &callee_uri(),
        &DialOptions::new("<sip:caller@test.example>", loopback()),
    )))
    .await
    .expect("the host answers");

    within(call.hang_up()).await.expect("the caller hangs up");
    assert!(call.is_ended(), "the call is over once the caller hangs up");
}

#[tokio::test]
async fn a_real_4xx_applies_the_declared_client_error_action_without_retry() {
    let (url, peer) = status_peer(vec![400]).await;
    Box::pin(refused_by_webhook(
        &webhook_document(&url, "on_4xx", 488, 1_000),
        488,
    ))
    .await;
    peer.await.expect("one request, then the peer ends");
}

#[tokio::test]
async fn real_5xx_retries_apply_the_declared_server_error_action_after_the_cap() {
    let (url, peer) = status_peer(vec![500, 503, 502]).await;
    Box::pin(refused_by_webhook(
        &webhook_document(&url, "on_5xx", 502, 1_000),
        502,
    ))
    .await;
    peer.await.expect("three attempts, then the peer ends");
}

/// A SIP endpoint that refuses every invitation it is sent with `486 Busy Here`.
///
/// The far end of the `dial` under test, and deliberately the smallest one there is: a 486 is a
/// final response to the INVITE transaction, so nothing here ever holds a call or a dialog.
async fn busy_peer() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let (handle, incoming) = endpoint().await;
    let address = handle.local_addr();
    let answering = handle.clone();
    let task = tokio::spawn(async move {
        let mut dispatcher = Dispatcher::new(answering.clone(), incoming);
        while let Some(event) = dispatcher.next().await {
            if let Dispatched::Invitation(invitation) = event {
                let _ = invitation.refuse(&answering, 486, "Busy Here").await;
            }
        }
    });
    (address, task)
}

/// A document app that answers the call, dials `target`, and hands back what it is told next.
///
/// Three callbacks: `call.incoming` gets the program, the event that completes `answer` gets an
/// empty document ("keep going"), and the third — whatever it turns out to be — is reported to the
/// test verbatim. Reported rather than asserted here so that the failure names the event the host
/// actually sent.
async fn dialing_app(
    target: &str,
) -> (
    String,
    oneshot::Receiver<String>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let url = format!("http://{}/hook", listener.local_addr().expect("address"));
    let program = format!(
        r#"{{"contract":"sipx.app.v1","instructions":[{{"id":"a1","do":"answer"}},{{"id":"d1","do":"dial","target":"{target}"}}]}}"#
    );
    let (third, third_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let mut third = Some(third);
        for turn in 0..3_u8 {
            let (mut socket, _) = listener.accept().await.expect("accepts a callback");
            let mut buffer = [0_u8; 4096];
            let mut body = Vec::new();
            loop {
                let read = socket.read(&mut buffer).await.expect("reads the envelope");
                assert_ne!(read, 0, "the callback request is complete");
                body.extend_from_slice(&buffer[..read]);
                // Every envelope carries exactly one event type, and the two the host can send
                // third are the outcome under test and the ending that stands in for it when the
                // effect was refused. Either one means the request has arrived whole.
                if turn < 2
                    || body
                        .windows(b"\"type\":".len())
                        .any(|window| window == b"\"type\":")
                {
                    break;
                }
                assert!(body.len() <= 64 * 1024, "the request stays bounded");
            }
            let reply = if turn == 0 {
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{program}",
                    program.len()
                )
            } else {
                "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned()
            };
            socket
                .write_all(reply.as_bytes())
                .await
                .expect("answers the callback");
            if turn == 2
                && let Some(third) = third.take()
            {
                let _ = third.send(String::from_utf8_lossy(&body).into_owned());
            }
        }
    });
    (url, third_rx, task)
}

/// **`M-103`** — an app is told how its `dial` resolved.
///
/// The whole of the story, end to end and through the shipped host: §6.2's `dial` reaches a far end
/// over a real socket, that end refuses with `486 Busy Here`, and §5.3's `call.dial.finished`
/// arrives at the app naming the instruction, the leg the interpreter minted for it, and `busy`.
///
/// Before `M-103` every part of this existed except the middle: the row, the type, the wire round
/// trip and the interpreter's handling of the event were all shipped and tested, and no host could
/// emit one — `Effect::Dial` fell into the driver's `_ => fail_effect()` arm, so what an app got
/// here was `call.ended`. That is what this asserts against, which is why the app reports the third
/// envelope rather than matching on it: the failure says which event arrived.
///
/// This is vector AC-7's claim against the product. §11's AC-7 drives the interpreter with a
/// `call.dial.finished` supplied as input, which is exactly the half that was never in doubt.
#[tokio::test]
async fn a_dial_refused_by_the_far_end_tells_the_app_its_leg_is_busy() {
    let (busy, peer) = busy_peer().await;
    let (url, third, app) = dialing_app(&format!("sip:bob@{busy}")).await;
    let address = webhook_host_on(&webhook_document(&url, "on_5xx", 500, 5_000)).await;
    let (caller, _incoming) = endpoint().await;

    let _call = Box::pin(within(dial(
        &caller,
        Target::udp(address),
        &callee_uri(),
        &DialOptions::new("<sip:caller@test.example>", loopback()),
    )))
    .await
    .expect("the app answers the inbound call");

    let envelope = within(third)
        .await
        .expect("the app receives a third event after the dial");
    assert!(
        envelope.contains("call.dial.finished"),
        "the app is told how its `dial` resolved; it was told this instead: {envelope}"
    );
    assert!(
        envelope.contains(r#""instruction_id":"d1""#),
        "the outcome names the instruction that asked for it: {envelope}"
    );
    assert!(
        envelope.contains(r#""leg":"b""#),
        "the outcome names the leg §5.2 put in the snapshot: {envelope}"
    );
    assert!(
        envelope.contains(r#""outcome":"busy""#),
        "486 is §5.3's `busy`, not a `rejected{{486}}`: {envelope}"
    );

    peer.abort();
    app.abort();
}

#[tokio::test]
async fn a_real_callback_timeout_applies_the_declared_timeout_action() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let url = format!("http://{}/hook", listener.local_addr().expect("address"));
    let peer = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.expect("accepts");
        std::future::pending::<()>().await;
    });
    Box::pin(refused_by_webhook(
        &webhook_document(&url, "on_timeout", 504, 50),
        504,
    ))
    .await;
    peer.abort();
}
