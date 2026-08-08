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

use sipx_call::{DialOptions, Error, answer, dial};
#[cfg(feature = "dtls")]
use sipx_call::{Keying, MediaPolicy, answer_with_policy};
use sipx_sip::build::ResponseBuilder;
use sipx_sip::{HeaderName, Host, HostName, Method, StatusCode, Uri};
use sipx_testkit::certs::Ca;
use sipx_transport::tls::{ClientTls, Identity, ServerTls, TrustAnchors};
use sipx_transport::{Config, Target, TransportKind, bind};

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("valid")
}

/// How long a test here waits for audio it played to arrive before calling it lost (`X-28`).
/// A bound on failure, not a window to measure in — see `MediaSession::record_at_least`.
const DELIVERY_BOUND: Duration = Duration::from_secs(10);

#[cfg(not(feature = "dtls"))]
#[tokio::test]
async fn selecting_dtls_without_the_feature_is_refused_without_a_fallback() {
    let (endpoint, _incoming) = bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");
    let to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
    let outcome = dial(
        &endpoint,
        Target::udp("127.0.0.1:9".parse().expect("valid")),
        &to,
        &DialOptions::new("<sip:caller@example.net>", loopback())
            .with_keying(sipx_call::Keying::DtlsSrtp),
    )
    .await;
    assert!(
        matches!(outcome, Err(Error::DtlsUnavailable)),
        "{outcome:?}"
    );
}

/// Failing-first for `M-28`: before the call-level selector and socket/key plumbing existed this
/// could not compile, and no INVITE built by `sipx-call` contained either asserted SDP field.
#[cfg(feature = "dtls")]
#[tokio::test]
async fn a_call_selected_for_dtls_srtp_offers_it_and_carries_encrypted_audio() {
    let (callee_endpoint, mut callee_incoming) =
        bind(Config::new("127.0.0.1:0".parse().expect("valid")))
            .await
            .expect("binds");
    let callee_addr = callee_endpoint.local_addr();
    let (caller_endpoint, _caller_rx) = bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");
    let policy = MediaPolicy::default().with_keying(Keying::DtlsSrtp);

    let answering = tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("an INVITE");
        let offer = String::from_utf8_lossy(incoming.request.body());
        assert!(
            offer.contains("m=audio ") && offer.contains(" UDP/TLS/RTP/SAVP "),
            "the selected call did not offer the DTLS-SRTP protocol: {offer}"
        );
        assert!(
            offer.contains("a=fingerprint:sha-256 "),
            "the selected call did not offer its certificate fingerprint: {offer}"
        );
        answer_with_policy(&callee_endpoint, &incoming, loopback(), policy)
            .await
            .expect("answers with DTLS-SRTP")
    });

    let to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
    let mut caller = tokio::time::timeout(
        Duration::from_secs(10),
        dial(
            &caller_endpoint,
            Target::udp(callee_addr),
            &to,
            &DialOptions::new("<sip:caller@example.net>", loopback()).with_keying(Keying::DtlsSrtp),
        ),
    )
    .await
    .expect("DTLS call did not finish within its failure bound")
    .expect("DTLS call connects");
    let callee = answering.await.expect("answer task finishes");

    assert!(caller.is_encrypted());
    assert!(callee.is_encrypted());
    let tone = vec![8000i16; 3200];
    let (_played, heard) = tokio::join!(
        caller.media().play(&tone, 160),
        callee.media().record_at_least(tone.len(), DELIVERY_BOUND),
    );
    assert!(heard.len() > 1600, "DTLS-keyed media carried no audio");
    assert!(
        matches!(
            caller.reinvite(sipx_sdp::Direction::SendOnly).await,
            Err(Error::DtlsRenegotiation)
        ),
        "a DTLS call must not renegotiate as plain RTP"
    );

    caller.media().stop();
    callee.media().stop();
}

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

    // `M-41`, end to end: the profile the two ends agreed on reaches the media session rather than
    // being discarded at the keying seam, and what they agree on is the *strongest* both offered —
    // not the interoperability floor, which is what a stack with one transform would have keyed
    // and what a selection honouring the offer's order would still reach.
    assert_eq!(
        caller.media().srtp_profile(),
        Some(sipx_rtp::srtp::Profile::AeadAes256Gcm),
        "the caller keyed the strongest profile both ends offered (RFC 7714)"
    );
    assert_eq!(
        callee.media().srtp_profile(),
        caller.media().srtp_profile(),
        "and both ends installed the same one; disagreeing here is a call that connects and \
         carries silence"
    );

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

/// An answer that echoes a tag this side never offered must end the call, not key it.
///
/// RFC 4568 §5.1.3: the offerer MUST verify that one of the crypto suites it offered *and its
/// accompanying tag* were accepted and echoed in the answer, and "if any of the above fails, the
/// negotiation MUST fail". sipx offers tag 1; this peer answers tag 9 with a key of its own, so
/// the two ends have agreed on nothing. The failure has to arrive as [`Error::Sdp`] naming the
/// tag — the two ways of failing that are not an error are both worse: an unencrypted call
/// presented as a secure one, or a stream dropped with no reason anyone can act on.
///
/// Driven from a hand-built `200` rather than through [`answer`], because sipx's own answerer
/// echoes the accepted tag correctly and could not produce this response.
#[tokio::test]
async fn an_answer_echoing_a_tag_that_was_never_offered_fails_the_call() {
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

    // The peer: a `200` whose `a=crypto` names tag 9, and a key that is perfectly well formed.
    // Nothing about it is malformed — the only thing wrong with it is that it answers an offer
    // nobody made, which is exactly what a check on the key material alone would miss.
    let answering = tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("an INVITE over WSS");
        assert_eq!(incoming.request.method, Method::Invite);
        let to = incoming
            .request
            .headers
            .value(&HeaderName::To)
            .expect("the INVITE has a To");
        let accepted = ResponseBuilder::to_request(
            &incoming.request,
            StatusCode::new(200).expect("valid"),
            "OK",
        )
        .expect("builds")
        .set_header(
            &HeaderName::To,
            format!("{};tag=unoffered", String::from_utf8_lossy(&to)),
        )
        .expect("to")
        .header(
            HeaderName::Contact,
            bytes::Bytes::from_static(b"<sip:callee@127.0.0.1>"),
        )
        .expect("contact")
        .header(
            HeaderName::ContentType,
            bytes::Bytes::from_static(b"application/sdp"),
        )
        .expect("content-type")
        .body(concat!(
            "v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n",
            "m=audio 41234 RTP/SAVP 0\r\na=rtpmap:0 PCMU/8000\r\n",
            // A published key (`docs/specs/srtp.md` §10.4), under a tag sipx never sends:
            // `Capabilities::with_srtp` fixes its own at 1.
            "a=crypto:9 AES_CM_128_HMAC_SHA1_80 ",
            "inline:PS1uQCVeeCFCanVmcjkpPywjNWhcYD0mXXtxaVBR\r\n",
            "a=sendrecv\r\n"
        ))
        .build();
        callee_endpoint
            .respond(&incoming.key, accepted)
            .await
            .expect("answers");
    });

    let to = Uri::sip(Host::Name(HostName::new("localhost").expect("valid")));
    let target = Target::new(wss_addr, TransportKind::Wss).verifying("localhost");
    let outcome = tokio::time::timeout(
        Duration::from_secs(10),
        dial(
            &caller_endpoint,
            target,
            &to,
            &DialOptions::new("<sip:caller@localhost>", loopback()),
        ),
    )
    .await
    .expect("no timeout");

    answering.await.expect("the answering side finishes");

    let error = outcome.err().unwrap_or_else(|| {
        panic!("a call keyed on an answer echoing a tag nobody offered was allowed to connect")
    });
    let Error::Sdp(reason) = &error else {
        panic!("the refusal reached the application as {error:?}, not as Error::Sdp");
    };
    assert!(
        reason.contains('9'),
        "the error does not say which tag came back: {reason}"
    );
    assert!(
        !reason.contains("PS1uQCVeeCFCanVmcjkpPywjNWhcYD0mXXtxaVBR"),
        "the error carries key material: {reason}"
    );
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
