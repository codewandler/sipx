//! A call whose signalling crosses a secure WebSocket.
//!
//! This is the exit criterion for WSS: not that a handshake succeeds, but that a whole call —
//! INVITE, 200, ACK, audio, BYE — works when every SIP message travels as a WebSocket frame
//! inside TLS, and when the caller has no address a peer could ever connect back to.
//!
//! The media stays on UDP, which is not an omission. RTP is not SIP; RFC 7118 carries the
//! signalling and says nothing about the audio, and a jitter buffer over a reliable ordered
//! transport would be a worse call, not a safer one.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
#![allow(clippy::similar_names)]
// A sine scaled to a third of full range cannot leave `i16`; the cast is the whole point.
#![allow(clippy::cast_possible_truncation)]

use std::net::IpAddr;
use std::time::Duration;

use sipx_call::{DialOptions, answer, dial};
use sipx_sip::{Host, HostName, Method, Uri};
use sipx_testkit::certs::Ca;
use sipx_transport::tls::{ClientTls, Identity, ServerTls, TrustAnchors};
use sipx_transport::{Config, Target, TransportKind, bind};

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("valid")
}

/// How long a test here waits for audio it played to arrive before calling it lost (`X-28`).
/// A bound on failure, not a window to measure in — see `MediaSession::record_at_least`.
const DELIVERY_BOUND: Duration = Duration::from_secs(10);

/// T-9's exit criterion.
#[tokio::test]
async fn a_call_establishes_over_wss() {
    let ca = Ca::new();
    let (cert, key) = ca.issue_for("localhost");
    let identity = Identity::from_pem(cert.as_bytes(), key.as_bytes()).expect("an identity");

    let mut callee_config = Config::new("127.0.0.1:0".parse().expect("valid"));
    callee_config.wss_server = Some((ServerTls::new(identity).expect("a server"), 0));
    let (callee_endpoint, mut callee_incoming) = bind(callee_config).await.expect("binds");
    let wss_addr = callee_endpoint.wss_addr().expect("a WSS port");

    // The caller trusts the fixture CA. An addition to its anchors — there is no way to say
    // "accept anything", so a mistake here fails the call rather than passing the test.
    let mut anchors = TrustAnchors::only();
    anchors.add_pem(ca.pem().as_bytes()).expect("a usable CA");
    let mut caller_config = Config::new("127.0.0.1:0".parse().expect("valid"));
    caller_config.tls_client = Some(ClientTls::new(&anchors).expect("a client"));
    let (caller_endpoint, _caller_incoming) = bind(caller_config).await.expect("binds");

    let answering = tokio::spawn(async move {
        let incoming = callee_incoming
            .recv()
            .await
            .expect("an INVITE arrives over WSS");
        assert_eq!(incoming.request.method, Method::Invite);
        assert_eq!(incoming.transport, TransportKind::Wss);
        let call = answer(&callee_endpoint, &incoming, loopback())
            .await
            .expect("answers");
        (call, incoming.request, callee_incoming)
    });

    let to = Uri::sip(Host::Name(HostName::new("localhost").expect("valid")));
    let target = Target::new(wss_addr, TransportKind::Wss).verifying("localhost");
    let mut caller = tokio::time::timeout(
        Duration::from_secs(10),
        dial(
            &caller_endpoint,
            target,
            &to,
            &DialOptions::new("<sip:caller@localhost>", loopback()),
        ),
    )
    .await
    .expect("no timeout")
    .expect("the call connects over WSS");

    let (mut callee, invite, mut callee_incoming) =
        answering.await.expect("the answering side finishes");

    // Both sides agree on the dialog, which means the ACK crossed — and the ACK is the first
    // message that would have gone astray had the `Contact` been trusted over the connection.
    assert_eq!(caller.dialog.id.call_id, callee.dialog.id.call_id);

    // The caller advertised what RFC 7118 §5.2 requires of a peer with no listening port: a
    // name that resolves nowhere, in both the `Via` and the `Contact`.
    let via = header(&invite, &sipx_sip::HeaderName::Via);
    assert!(via.contains("SIP/2.0/WSS "), "{via}");
    assert!(via.contains(".invalid"), "{via}");

    let contact = header(&invite, &sipx_sip::HeaderName::Contact);
    assert!(contact.contains(".invalid"), "{contact}");
    assert!(contact.contains("transport=wss"), "{contact}");

    // Audio crosses, on UDP, while the signalling stays inside TLS.
    let tone: Vec<i16> = (0..800)
        .map(|i| ((f64::from(i) / 8000.0 * 440.0 * std::f64::consts::TAU).sin() * 12000.0) as i16)
        .collect();
    caller.media().play(&tone, 160).await;
    let heard = callee
        .media()
        .record_at_least(tone.len(), DELIVERY_BOUND)
        .await;
    assert!(!heard.is_empty(), "the call must carry audio");

    // And the BYE arrives, which is the assertion the `Contact` would have broken: it is sent
    // to a name that resolves nowhere, so it can only have travelled back over the connection.
    caller.hang_up().await.expect("hangs up");
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        // The ACK is on this channel too, and reaches the call the same way; drain until the
        // call reports itself over rather than assuming which message comes first.
        while let Some(incoming) = callee_incoming.recv().await {
            assert!(
                callee.handle(&incoming).await.expect("handles"),
                "{:?} belongs to this call",
                incoming.request.method
            );
            if callee.is_ended() {
                return incoming.request.method;
            }
        }
        panic!("the connection closed before the BYE arrived");
    })
    .await
    .expect("a BYE arrives over the same websocket");

    assert_eq!(ended, Method::Bye);
    assert!(callee.is_ended(), "the call must be over on both sides");
}

fn header(request: &sipx_sip::Request, name: &sipx_sip::HeaderName) -> String {
    String::from_utf8_lossy(&request.headers.value(name).expect("the header is present"))
        .into_owned()
}
