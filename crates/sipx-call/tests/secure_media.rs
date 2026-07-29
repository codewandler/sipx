//! A whole call over WSS, with the media encrypted because the signalling was.
//!
//! This is the criterion that ties the pieces together: SRTP is only reachable if the SDP
//! negotiated it, and SDES only offers a key if the signalling protects one. A test of the
//! transform alone would pass with none of that wired up.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
#![allow(clippy::similar_names)]

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

/// A call whose signalling is encrypted must encrypt its media too — that is the whole point of
/// the story, and the thing `sips:` on its own does not give you.
#[tokio::test]
async fn a_call_over_secure_signalling_encrypts_its_media() {
    let ca = Ca::new();
    let (cert, key) = ca.issue_for("localhost");
    let identity = Identity::from_pem(cert.as_bytes(), key.as_bytes()).expect("an identity");

    let mut callee_config = Config::new("127.0.0.1:0".parse().expect("valid"));
    callee_config.wss_server = Some((ServerTls::new(identity).expect("a server"), 0));
    let (callee_endpoint, mut callee_incoming) = bind(callee_config).await.expect("binds");
    let wss_addr = callee_endpoint.wss_addr().expect("a WSS port");

    let mut anchors = TrustAnchors::only();
    anchors.add_pem(ca.pem().as_bytes()).expect("a usable CA");
    let mut caller_config = Config::new("127.0.0.1:0".parse().expect("valid"));
    caller_config.tls_client = Some(ClientTls::new(&anchors).expect("a client"));
    let (caller_endpoint, _caller_rx) = bind(caller_config).await.expect("binds");

    let answering = tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("an INVITE over WSS");
        assert_eq!(incoming.request.method, Method::Invite);
        answer(&callee_endpoint, &incoming, loopback())
            .await
            .expect("answers")
    });

    let to = Uri::sip(Host::Name(HostName::new("localhost").expect("valid")));
    let target = Target::new(wss_addr, TransportKind::Wss).verifying("localhost");
    let caller = tokio::time::timeout(
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
    .expect("the call connects");

    let callee = answering.await.expect("the answering side finishes");

    assert!(
        caller.is_encrypted(),
        "a call over WSS must have negotiated SRTP"
    );
    assert!(callee.is_encrypted(), "and so must the far end");

    // And the encryption is real: audio crosses it.
    let tone: Vec<i16> = (0..8000).map(|_| 8000i16).collect();
    let (_played, heard) = tokio::join!(
        caller.media().play(&tone, 160),
        callee.media().record_at_least(tone.len(), DELIVERY_BOUND),
    );
    assert!(
        heard.len() > 1600,
        "encrypted media must still be media: {} samples",
        heard.len()
    );

    caller.media().stop();
    callee.media().stop();
}

/// And the converse, which is what stops the test above passing for the wrong reason: a call over
/// cleartext SIP does **not** carry a key, because a key in an SDP body over cleartext signalling
/// is readable by anyone on the path (RFC 4568 §7.1).
#[tokio::test]
async fn a_call_over_cleartext_signalling_does_not_pretend_to_encrypt() {
    let (callee_endpoint, mut callee_incoming) =
        bind(Config::new("127.0.0.1:0".parse().expect("valid")))
            .await
            .expect("binds");
    let callee_addr = callee_endpoint.local_addr();
    let (caller_endpoint, _rx) = bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");

    let answering = tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("an INVITE");
        answer(&callee_endpoint, &incoming, loopback())
            .await
            .expect("answers")
    });

    let to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
    let caller = dial(
        &caller_endpoint,
        Target::udp(callee_addr),
        &to,
        &DialOptions::new("<sip:caller@example.net>", loopback()),
    )
    .await
    .expect("the call connects");
    let callee = answering.await.expect("finishes");

    assert!(
        !caller.is_encrypted(),
        "offering a key over cleartext SIP would publish it"
    );
    assert!(!callee.is_encrypted());

    // The call still works. Refusing to encrypt is not refusing to call.
    let tone: Vec<i16> = (0..8000).map(|_| 8000i16).collect();
    let (_played, heard) = tokio::join!(
        caller.media().play(&tone, 160),
        callee.media().record_at_least(tone.len(), DELIVERY_BOUND),
    );
    assert!(heard.len() > 1600, "{} samples", heard.len());

    caller.media().stop();
    callee.media().stop();
}
