//! The general-purpose secure WebSocket client, held to `A-20`'s contract.
//!
//! What these prove is not that TLS or RFC 6455 work — the libraries' own tests do that — but
//! that the client asks them the right questions: the workspace's one certificate policy is the
//! one refusing a wrong peer, the caller's headers travel, no subprotocol is invented, an
//! oversize message is a typed error rather than an allocation, and a silent peer surfaces as a
//! typed close within its bound.
//!
//! Every certificate is generated per run by `cargo run -p sipx-testkit --example issue-certs`
//! into a directory outside the repository, so nothing here depends on a public CA, a clock
//! beyond the test's control, or committed certificate material.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use sipx_app::wss::{WssClient, WssClientConfig, WssError, WssMessage, WssRequest};
use sipx_transport::tls::{ClientTls, Identity, ServerTls, TlsError, TrustAnchors};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

/// Certificates issued per run by the workspace's one fixture authority.
///
/// The example is the same issuer the interop harness uses, run the same way, so these tests
/// cannot drift from it — and the directory lives under the system temp dir, never the tree.
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn issue(host: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "sipx-a20-certs-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
        let status = Command::new(cargo)
            .args([
                "run",
                "--quiet",
                "-p",
                "sipx-testkit",
                "--example",
                "issue-certs",
                "--",
            ])
            .arg(&dir)
            .arg(host)
            .status()
            .expect("issue-certs runs");
        assert!(status.success(), "issue-certs must issue for {host}");
        Self { dir }
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.dir.join(name))
            .unwrap_or_else(|error| panic!("reading {name}: {error}"))
    }

    fn ca(&self) -> String {
        self.read("ca.pem")
    }

    fn cert(&self) -> String {
        self.read("server.pem")
    }

    fn key(&self) -> String {
        self.read("server.key")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Best-effort: the material must not outlive the run, and never enters the repository.
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// What the server saw in the upgrade request.
#[derive(Default)]
struct Upgrade {
    authorization: Option<String>,
    subprotocol: Option<String>,
}

type Captured = Arc<Mutex<Option<Upgrade>>>;

fn capture(request: &Request, seen: &Captured) {
    let header = |name: &str| {
        request
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    *seen.lock().expect("no poisoned capture") = Some(Upgrade {
        authorization: header("authorization"),
        subprotocol: header("sec-websocket-protocol"),
    });
}

/// A loopback WSS echo server presenting `cert`/`key`, capturing what each upgrade carried.
// The callback's refusal type is an HTTP response and is as large as one; the dependency fixes
// that return type, exactly as the transport's own server handshake notes.
#[allow(clippy::result_large_err)]
async fn wss_echo_server(cert: &str, key: &str) -> (SocketAddr, Captured) {
    let identity = Identity::from_pem(cert.as_bytes(), key.as_bytes()).expect("identity");
    let server = ServerTls::new(identity).expect("server policy");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let addr = listener.local_addr().expect("an address");
    let captured: Captured = Arc::default();
    let seen = Arc::clone(&captured);
    let acceptor = server.acceptor();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            // A refused TLS handshake (the wrong-name and unknown-issuer vectors) ends here;
            // the client's typed error is the assertion, not anything this task does.
            let Ok(tls) = acceptor.accept(stream).await else {
                continue;
            };
            let seen = Arc::clone(&seen);
            let Ok(mut socket) = tokio_tungstenite::accept_hdr_async(
                tls,
                move |request: &Request, response: Response| {
                    capture(request, &seen);
                    Ok(response)
                },
            )
            .await
            else {
                continue;
            };
            while let Some(Ok(frame)) = socket.next().await {
                match frame {
                    Message::Text(text) => {
                        if socket.send(Message::Text(text)).await.is_err() {
                            break;
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        }
    });
    (addr, captured)
}

fn trusting(ca_pem: &str) -> ClientTls {
    let mut anchors = TrustAnchors::only();
    anchors.add_pem(ca_pem.as_bytes()).expect("a usable CA");
    ClientTls::new(&anchors).expect("a client policy")
}

/// The TLS refusal a connect attempt produced, as the existing typed error's detail.
async fn tls_refusal(client: &WssClient, url: &str) -> String {
    let error = client
        .connect(WssRequest::new(url))
        .await
        .expect_err("the handshake must be refused");
    match error {
        WssError::Tls(TlsError::Handshake { peer, detail }) => {
            assert_eq!(peer, "localhost", "the peer named is the one dialed");
            detail
        }
        other => panic!("the refusal must be the existing typed TLS error, got: {other}"),
    }
}

/// A-20's failing-first vector: a valid certificate for somebody else, from a CA the client
/// trusts, is still the wrong peer — refused by the same `ClientTls` policy every other TLS
/// client in the workspace verifies with, as the existing typed error.
#[tokio::test]
async fn a_wrong_name_certificate_is_refused_with_the_existing_typed_error() {
    let elsewhere = Fixture::issue("elsewhere.example.com");
    let (addr, _) = wss_echo_server(&elsewhere.cert(), &elsewhere.key()).await;
    let client = WssClient::new(trusting(&elsewhere.ca()));

    let detail = tls_refusal(&client, &format!("wss://localhost:{}/", addr.port())).await;
    assert!(!detail.is_empty(), "the refusal must say what failed");
}

/// The companion vector: a certificate for the right name that nobody vouches for.
#[tokio::test]
async fn an_unknown_issuer_certificate_is_refused_with_the_existing_typed_error() {
    let trusted = Fixture::issue("localhost");
    let stranger = Fixture::issue("localhost");
    let (addr, _) = wss_echo_server(&stranger.cert(), &stranger.key()).await;
    let client = WssClient::new(trusting(&trusted.ca()));

    let detail = tls_refusal(&client, &format!("wss://localhost:{}/", addr.port())).await;
    assert!(!detail.is_empty(), "the refusal must say what failed");
}

/// Wrong name and unknown issuer are two operational problems with two fixes; the spec's
/// requirement (`docs/specs/sip-tls.md` §6) is that they are tellable apart, not their wording.
#[tokio::test]
async fn the_two_refusals_are_tellable_apart() {
    let trusted = Fixture::issue("localhost");
    let elsewhere = Fixture::issue("elsewhere.example.com");
    let (addr, _) = wss_echo_server(&elsewhere.cert(), &elsewhere.key()).await;

    let wrong_name = trusting(&elsewhere.ca());
    let unknown_issuer = trusting(&trusted.ca());
    let url = format!("wss://localhost:{}/", addr.port());

    let mismatch = tls_refusal(&WssClient::new(wrong_name), &url).await;
    let untrusted = tls_refusal(&WssClient::new(unknown_issuer), &url).await;
    assert_ne!(
        mismatch, untrusted,
        "a wrong host and an untrusted issuer must not report the same thing"
    );
}

/// The whole composed path: TLS from `ClientTls`, RFC 6455 handshake on top, the caller's
/// `Authorization` header delivered, no subprotocol offered because none was named, and a
/// message echoed both ways.
#[tokio::test]
async fn a_trusted_peer_is_reached_with_the_callers_headers_and_no_invented_subprotocol() {
    let fixture = Fixture::issue("localhost");
    let (addr, captured) = wss_echo_server(&fixture.cert(), &fixture.key()).await;
    let client = WssClient::new(trusting(&fixture.ca()));

    let request = WssRequest::new(format!("wss://localhost:{}/v1/realtime", addr.port()))
        .header("Authorization", "Bearer fixture-token")
        .expect("a usable header");
    let mut connection = client
        .connect(request)
        .await
        .expect("the composed handshake");

    connection
        .send_text("over the one TLS policy")
        .await
        .expect("sends");
    let echoed = connection.next().await.expect("an echo");
    assert_eq!(
        echoed,
        Some(WssMessage::Text("over the one TLS policy".to_owned()))
    );

    let seen = captured.lock().expect("no poisoned capture");
    let seen = seen.as_ref().expect("the server saw the upgrade");
    assert_eq!(
        seen.authorization.as_deref(),
        Some("Bearer fixture-token"),
        "the caller's Authorization header must travel"
    );
    assert_eq!(
        seen.subprotocol, None,
        "no subprotocol is offered unless the caller names one"
    );
}

/// A loopback plain-WebSocket echo server, capturing each upgrade. `echo_subprotocol` is what a
/// peer that agrees to whatever is offered does; `false` mimics one that names nothing back.
// The callback's refusal type is an HTTP response and is as large as one; the dependency fixes
// that return type, exactly as the transport's own server handshake notes.
#[allow(clippy::result_large_err)]
async fn ws_echo_server(echo_subprotocol: bool) -> (SocketAddr, Captured) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let addr = listener.local_addr().expect("an address");
    let captured: Captured = Arc::default();
    let seen = Arc::clone(&captured);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let seen = Arc::clone(&seen);
            let Ok(mut socket) = tokio_tungstenite::accept_hdr_async(
                stream,
                move |request: &Request, mut response: Response| {
                    capture(request, &seen);
                    if echo_subprotocol
                        && let Some(offered) = request.headers().get("sec-websocket-protocol")
                    {
                        response
                            .headers_mut()
                            .insert("sec-websocket-protocol", offered.clone());
                    }
                    Ok(response)
                },
            )
            .await
            else {
                continue;
            };
            while let Some(Ok(frame)) = socket.next().await {
                match frame {
                    Message::Text(text) => {
                        if socket.send(Message::Text(text)).await.is_err() {
                            break;
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        }
    });
    (addr, captured)
}

fn plain_client() -> WssClient {
    // Cleartext needs no anchors, but the client carries the policy either way — the system
    // store is what a caller with no fixture CA would hold.
    WssClient::new(ClientTls::new(&TrustAnchors::system()).expect("a client policy"))
}

/// Cleartext to loopback is permitted — A-21's stand-in peer and these tests run without
/// certificates where the spec allows it.
#[tokio::test]
async fn cleartext_to_loopback_is_permitted() {
    let (addr, _) = ws_echo_server(false).await;
    let mut connection = plain_client()
        .connect(WssRequest::new(format!("ws://{addr}/")))
        .await
        .expect("loopback cleartext");
    connection
        .send_text("on the loopback")
        .await
        .expect("sends");
    assert_eq!(
        connection.next().await.expect("an echo"),
        Some(WssMessage::Text("on the loopback".to_owned()))
    );
}

/// Cleartext anywhere else is refused by name, before any connection is attempted.
#[tokio::test]
async fn cleartext_to_a_non_loopback_host_is_refused() {
    for url in [
        "ws://example.com/",
        "ws://192.0.2.1/",
        "ws://198.51.100.7:8080/x",
    ] {
        // The bound on failure: a correct refusal returns without touching the network, and
        // 192.0.2.1 (TEST-NET-1) would otherwise blackhole the connect attempt.
        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            plain_client().connect(WssRequest::new(url)),
        )
        .await
        .expect("the refusal must not reach for the network");
        match outcome {
            Err(WssError::Cleartext { host }) => {
                assert!(url.contains(&host), "{url} refused as {host}");
            }
            other => panic!("{url} must be refused as cleartext, got: {other:?}"),
        }
    }
}

/// A subprotocol is offered exactly when the caller names one, and the agreement is two-sided.
#[tokio::test]
async fn a_named_subprotocol_is_offered_and_its_echo_required() {
    let (addr, captured) = ws_echo_server(true).await;
    let request = WssRequest::new(format!("ws://{addr}/")).subprotocol("chat.v1");
    plain_client()
        .connect(request)
        .await
        .expect("an agreeing peer");
    {
        // A block rather than `drop`, so the guard provably never crosses the awaits below.
        let seen = captured.lock().expect("no poisoned capture");
        assert_eq!(
            seen.as_ref()
                .expect("the server saw the upgrade")
                .subprotocol
                .as_deref(),
            Some("chat.v1"),
            "the named subprotocol must be offered"
        );
    }

    // A peer that upgrades without agreeing has agreed to nothing (RFC 6455 §4.1). The refusal
    // is the handshake's own; asserting the *variant* is what pins it — were the dependency to
    // stop refusing, the connect would succeed here rather than change its error string.
    let (silent, _) = ws_echo_server(false).await;
    let request = WssRequest::new(format!("ws://{silent}/")).subprotocol("chat.v1");
    match plain_client().connect(request).await {
        Err(WssError::Subprotocol { offered, .. }) => assert_eq!(offered, "chat.v1"),
        other => panic!("a peer that names nothing back must be the typed refusal, got: {other:?}"),
    }
}

/// RFC 6455 §5.2: the handshake installs the configured bound in the decoder, so an oversize
/// message is refused as a typed error before it is allocated.
#[tokio::test]
async fn an_oversize_message_is_a_typed_error() {
    const HELD: usize = 1024;
    const SENT: usize = 8192;
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let addr = listener.local_addr().expect("an address");
    tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let Ok(mut socket) = tokio_tungstenite::accept_async(stream).await else {
            return;
        };
        // The permissive peer sends more than the client's bound and holds the socket open.
        let _ = socket.send(Message::binary(vec![0u8; SENT])).await;
        let _ = socket.next().await;
    });

    let config = WssClientConfig {
        max_message_bytes: HELD,
        ..WssClientConfig::default()
    };
    let mut connection = WssClient::with_config(
        ClientTls::new(&TrustAnchors::system()).expect("a client policy"),
        config,
    )
    .connect(WssRequest::new(format!("ws://{addr}/")))
    .await
    .expect("the handshake");

    match connection.next().await {
        Err(WssError::Oversize { size, limit, .. }) => {
            assert_eq!(size, SENT);
            assert_eq!(limit, HELD);
        }
        other => panic!("an oversize message must be the typed refusal, got: {other:?}"),
    }
}

/// RFC 6455 §5.5.2: a peer's Ping is answered while the caller is simply reading.
#[tokio::test]
async fn a_peers_ping_is_answered_with_its_payload() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let addr = listener.local_addr().expect("an address");
    let probed = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("a connection");
        let mut socket = tokio_tungstenite::accept_async(stream)
            .await
            .expect("upgrades");
        socket
            .send(Message::Ping("liveness probe".into()))
            .await
            .expect("the probe is sent");
        while let Some(Ok(frame)) = socket.next().await {
            if let Message::Pong(payload) = frame {
                assert_eq!(
                    payload.as_ref(),
                    b"liveness probe",
                    "RFC 6455 \u{a7}5.5.3: the Pong carries the Ping's payload"
                );
                socket
                    .send(Message::Text("after the pong".into()))
                    .await
                    .expect("the marker");
                // The task must end for the join below to complete; the drop closes the
                // socket, which is what tells the client there is nothing more to read.
                return;
            }
        }
        panic!("the connection ended before a Pong arrived");
    });

    // A short cadence, because the client's own probe is also what flushes its queued Pong
    // when the protocol layer defers the write — the test bounds how long that can take.
    let config = WssClientConfig {
        ping_interval: Duration::from_millis(100),
        ..WssClientConfig::default()
    };
    let mut connection = WssClient::with_config(
        ClientTls::new(&TrustAnchors::system()).expect("a client policy"),
        config,
    )
    .connect(WssRequest::new(format!("ws://{addr}/")))
    .await
    .expect("the handshake");
    // `next` returns the marker the server only sends after seeing the Pong, so reaching this
    // assertion is the proof the Ping was answered.
    assert_eq!(
        connection.next().await.expect("the marker"),
        Some(WssMessage::Text("after the pong".to_owned()))
    );
    probed.await.expect("the server's assertions hold");
}

/// The session-binding discipline, client side: a peer silent past the liveness bound is a
/// typed close, not a hang.
#[tokio::test]
async fn a_silent_peer_surfaces_as_a_typed_close_within_its_bound() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let addr = listener.local_addr().expect("an address");
    let held = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("a connection");
        let socket = tokio_tungstenite::accept_async(stream)
            .await
            .expect("upgrades");
        // Held open and never polled again: the TCP connection lives, but nothing — not even
        // the protocol layer's automatic Pong — will ever be written back.
        std::future::pending::<()>().await;
        drop(socket);
    });

    let config = WssClientConfig {
        ping_interval: Duration::from_millis(50),
        ping_grace: Duration::from_millis(50),
        ..WssClientConfig::default()
    };
    let mut connection = WssClient::with_config(
        ClientTls::new(&TrustAnchors::system()).expect("a client policy"),
        config,
    )
    .connect(WssRequest::new(format!("ws://{addr}/")))
    .await
    .expect("the handshake");

    // The outer duration bounds the failure: a correct client returns the typed close on its
    // own ~100 ms schedule, and this only decides when a hanging one is declared broken.
    let outcome = tokio::time::timeout(Duration::from_secs(10), connection.next())
        .await
        .expect("the liveness bound must fire long before this");
    match outcome {
        Err(WssError::Stalled { bound, .. }) => {
            assert_eq!(bound, Duration::from_millis(50));
        }
        other => panic!("a silent peer must be the typed close, got: {other:?}"),
    }
    held.abort();
}

/// A URL the client cannot dial is a typed refusal, not a guess.
#[tokio::test]
async fn an_unusable_url_is_refused_by_name() {
    for url in ["https://example.com/", "not a url", "wss:///nohost"] {
        match plain_client().connect(WssRequest::new(url)).await {
            Err(WssError::Url { url: named, .. }) => assert_eq!(named, url),
            other => panic!("{url} must be refused as a URL, got: {other:?}"),
        }
    }
}

/// The `Debug` of a request must not leak what its headers carry — `Authorization` holds a
/// bearer token, and a `Debug` that printed it would put it in whatever log the caller writes.
#[test]
fn a_requests_debug_does_not_leak_header_values() {
    let request = WssRequest::new("wss://localhost/")
        .header("Authorization", "Bearer super-secret")
        .expect("a usable header");
    let printed = format!("{request:?}");
    assert!(
        !printed.contains("super-secret"),
        "the header value leaked: {printed}"
    );
    assert!(
        printed.contains("authorization"),
        "the header name is not the secret and should be visible: {printed}"
    );
}

/// RFC 3986 §3.2.1 userinfo is refused before anything is dialed, and no byte of it reaches the
/// error — a credential in a URL is a credential in whatever log the caller writes.
#[tokio::test]
async fn a_url_carrying_credentials_is_refused_before_it_can_reach_the_wire() {
    for url in [
        "wss://user:sekret@example.com/v1",
        "ws://user:sekret@localhost:9000/v1",
    ] {
        // The bound on failure: a correct refusal returns without touching the network, so a
        // client that dialed anyway is declared broken here instead of hanging the suite.
        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            plain_client().connect(WssRequest::new(url)),
        )
        .await
        .expect("the refusal must not reach for the network");
        match outcome {
            Err(error @ WssError::Url { .. }) => {
                assert!(
                    !format!("{error}").contains("sekret"),
                    "Display carried it: {error}"
                );
                assert!(
                    !format!("{error:?}").contains("sekret"),
                    "Debug carried it: {error:?}"
                );
            }
            other => panic!("{url} must be refused as a URL, got: {other:?}"),
        }
    }
}

/// The same hazard by the other route: a request that is printed but never dialed.
#[test]
fn a_requests_debug_redacts_credentials_written_into_the_url() {
    let printed = format!("{:?}", WssRequest::new("wss://user:sekret@example.com/v1"));
    assert!(
        !printed.contains("sekret"),
        "the URL's credential leaked: {printed}"
    );
    assert!(
        printed.contains("***@example.com"),
        "the host it names is not the secret and should stay readable: {printed}"
    );
}

/// The other half of the liveness rule: an answering peer is *not* declared gone. A probe that
/// was never stood down would stall this connection one grace after the first Ping, so surviving
/// several cadences is what proves the Pong is being acted on.
#[tokio::test]
async fn a_responsive_peer_survives_past_a_full_ping_cycle() {
    const ROUNDS: u32 = 3;
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let addr = listener.local_addr().expect("an address");
    let peer = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("a connection");
        let mut socket = tokio_tungstenite::accept_async(stream)
            .await
            .expect("upgrades");
        let mut probes = 0;
        while let Some(Ok(frame)) = socket.next().await {
            if let Message::Ping(payload) = frame {
                probes += 1;
                // Answered explicitly rather than left to the queued automatic Pong, so the
                // reply is flushed when it is owed and this peer's liveness is the test's own.
                socket
                    .send(Message::Pong(payload))
                    .await
                    .expect("an answer");
                if probes == ROUNDS {
                    socket
                        .send(Message::Text("still here".into()))
                        .await
                        .expect("the marker");
                    return;
                }
            }
        }
        panic!("the connection ended after {probes} probes");
    });

    let config = WssClientConfig {
        ping_interval: Duration::from_millis(40),
        // Generous against the cadence on purpose: this is the duration a broken stand-down
        // would fire on, and it must be long enough that load cannot fire it on its own.
        ping_grace: Duration::from_millis(750),
        ..WssClientConfig::default()
    };
    let mut connection = WssClient::with_config(
        ClientTls::new(&TrustAnchors::system()).expect("a client policy"),
        config,
    )
    .connect(WssRequest::new(format!("ws://{addr}/")))
    .await
    .expect("the handshake");

    // The bound on failure: the marker is due after three cadences, and this only decides when
    // a connection that stalled instead is declared broken.
    match tokio::time::timeout(Duration::from_secs(10), connection.next()).await {
        Ok(Ok(Some(WssMessage::Text(text)))) => assert_eq!(text, "still here"),
        other => panic!("a peer answering every probe must not be declared gone, got: {other:?}"),
    }
    peer.await.expect("the peer's assertions hold");
}

/// A Pong that arrived while the caller was away from `next` is still an answer. Both `select!`
/// branches are ready on re-entry, so an implementation that only ever consults the clock reports
/// a stall with the answer sitting in the socket — a lie a caller cannot tell from a dead peer.
#[tokio::test]
async fn a_pong_that_arrived_while_the_caller_was_away_is_not_a_stall() {
    const ROUNDS: u32 = 6;
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let addr = listener.local_addr().expect("an address");
    let peer = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("a connection");
        let mut socket = tokio_tungstenite::accept_async(stream)
            .await
            .expect("upgrades");
        while let Some(Ok(frame)) = socket.next().await {
            if let Message::Ping(payload) = frame {
                // Ordering two stimuli: the caller's departure from `next` and this answer's
                // arrival. The wait puts the Pong on the wire after the caller has gone, which
                // is the whole window under test, and a longer wait preserves that order.
                tokio::time::sleep(Duration::from_millis(60)).await;
                socket
                    .send(Message::Pong(payload))
                    .await
                    .expect("an answer");
                socket
                    .send(Message::Text("still here".into()))
                    .await
                    .expect("the marker");
            }
        }
    });

    let config = WssClientConfig {
        ping_interval: Duration::from_millis(40),
        ping_grace: Duration::from_millis(60),
        ..WssClientConfig::default()
    };
    let mut connection = WssClient::with_config(
        ClientTls::new(&TrustAnchors::system()).expect("a client policy"),
        config,
    )
    .connect(WssRequest::new(format!("ws://{addr}/")))
    .await
    .expect("the handshake");

    // Once would catch a client that always consults the clock; an unbiased choice between two
    // ready branches has to be caught repeatedly, and six rounds leave it under two percent.
    for round in 1..=ROUNDS {
        // The bound on the caller's presence: it leaves `next` after the probe goes out at
        // ~40 ms and long before the answer can arrive at ~100 ms.
        let _ = tokio::time::timeout(Duration::from_millis(60), connection.next()).await;
        // Ordering two stimuli: the answer's arrival and the caller's return. The wait puts the
        // return after both the Pong and the expiry of the 60 ms grace — the state the window
        // needs — and a longer wait preserves that order rather than breaking it.
        tokio::time::sleep(Duration::from_millis(120)).await;
        // The bound on failure: the marker is already buffered, so this only decides when a
        // client that stalled on it instead is declared broken.
        match tokio::time::timeout(Duration::from_secs(5), connection.next()).await {
            Ok(Ok(Some(WssMessage::Text(text)))) => assert_eq!(text, "still here"),
            other => panic!("round {round}: an answered probe must not stall, got: {other:?}"),
        }
    }
    peer.abort();
}

/// The other side of the same rule, and the one a burst can hide: only a Pong stands a probe
/// down. A peer that streams at full rate while never answering is dead by the session-binding
/// discipline, and must be declared so at the grace — not held open for as long as it keeps
/// talking. A client that let any ready data defer the verdict would never produce it here.
#[tokio::test]
async fn a_peer_that_streams_without_answering_probes_is_still_declared_gone() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let addr = listener.local_addr().expect("an address");
    let peer = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("a connection");
        let mut socket = tokio_tungstenite::accept_async(stream)
            .await
            .expect("upgrades");
        // Never reads, so it never sees a Ping and never answers one — the protocol layer's
        // automatic Pong needs a read to be queued at all. It only talks.
        loop {
            if socket
                .send(Message::Text("filling the socket".into()))
                .await
                .is_err()
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    });

    let config = WssClientConfig {
        ping_interval: Duration::from_millis(50),
        ping_grace: Duration::from_millis(100),
        ..WssClientConfig::default()
    };
    let mut connection = WssClient::with_config(
        ClientTls::new(&TrustAnchors::system()).expect("a client policy"),
        config,
    )
    .connect(WssRequest::new(format!("ws://{addr}/")))
    .await
    .expect("the handshake");

    // Drive `next` long enough for the probe to go out at ~50 ms. The peer's frames are always
    // ready, so this also proves the burst does not stop the probe being sent.
    let mut delivered = 0_usize;
    let _ = tokio::time::timeout(Duration::from_millis(70), async {
        while connection.next().await.is_ok() {
            delivered += 1;
        }
    })
    .await;
    assert!(
        delivered > 0,
        "the peer must really have been streaming for this to prove anything"
    );

    // Ordering two stimuli: the grace's expiry at ~150 ms and the caller's return. The wait
    // puts the return after the expiry with the peer still talking — the state under test —
    // and a longer wait preserves that order rather than breaking it.
    tokio::time::sleep(Duration::from_millis(120)).await;

    // The verdict is owed *now*, and a backlog of data is not an answer: the very next thing
    // this caller is handed must be the typed close. A client that let ready data defer it
    // would hand back another message here, and would go on doing so for as long as the peer
    // kept the socket readable — which is the bound `session-binding.md` does not allow.
    // The bound on failure only decides when one that never answers at all is declared broken.
    match tokio::time::timeout(Duration::from_secs(5), connection.next())
        .await
        .expect("the liveness bound must fire long before this")
    {
        Err(WssError::Stalled { bound, .. }) => assert_eq!(bound, Duration::from_millis(100)),
        other => panic!("a burst must not defer the verdict, got: {other:?}"),
    }
    peer.abort();
}

/// One TLS policy, not two: the workspace's WebSocket dependency is built without TLS features,
/// asserted the way the workspace `Cargo.toml` comment states it.
#[test]
fn the_websocket_dependency_keeps_its_no_tls_stance() {
    let manifest =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml"))
            .expect("the workspace manifest");
    let line = manifest
        .lines()
        .find(|line| line.trim_start().starts_with("tokio-tungstenite = "))
        .expect("the dependency is declared once, in the workspace");
    assert!(
        line.contains("default-features = false"),
        "the dependency must not choose features for the workspace: {line}"
    );
    assert!(
        line.contains(r#"features = ["handshake"]"#),
        "the handshake is the only feature; TLS is the workspace's own `tls` module: {line}"
    );
    assert!(
        !line.to_ascii_lowercase().contains("tls"),
        "a TLS feature here would be a second certificate policy: {line}"
    );
}
