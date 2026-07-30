//! The version floor, evidenced by a refused handshake (RFC 8996).
//!
//! `docs/specs/sip-tls.md` §3.5 says TLS 1.2 is the floor and 1.0 and 1.1 are excluded, and §6
//! vector L9 says a 1.1 offer is refused. Until this file, nothing ran that vector — the claim in
//! `docs/rfc/registry.toml` cited the sentence rather than the behaviour, which is the one kind of
//! evidence that cannot fail.
//!
//! **The floor is a property of the TLS library, not of sipx.** sipx implements no TLS: it hands
//! `tokio-rustls` a `ClientConfig`/`ServerConfig` built from the library's default version set, and
//! that set is `{1.3, 1.2}` because the library offers nothing older to select. There is therefore
//! no code in this workspace that *decides* the floor, and a checker looking for one would find
//! nothing — which is why the claim has to be pinned by an observed refusal and by the version set
//! itself. Swap the backend for one that still speaks 1.0 and the tests below are what goes red.
//!
//! **A negative obligation needs an attempt, not an assertion.** So these tests do not use
//! `ClientTls` — it cannot offer a deprecated version, which is the property under test and no
//! way to test it. They write a `ClientHello` byte by byte, which needs nothing this crate does
//! not already depend on, and read what the listener answers.

#![cfg(feature = "tls")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use sipx_testkit::certs::{Ca, dns};
use sipx_transport::tls::{Identity, ServerTls};
use sipx_transport::{Config, bind};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// `ClientHello.client_version` on the wire, for the versions this file is about.
const TLS_1_0: u16 = 0x0301;
const TLS_1_1: u16 = 0x0302;
const TLS_1_2: u16 = 0x0303;

/// Record content types (RFC 8446 §5.1).
const ALERT: u8 = 21;
const HANDSHAKE: u8 = 22;

/// `fatal`, and `protocol_version` (RFC 8446 §6.2): "the protocol version the peer attempted to
/// negotiate is recognized but not supported".
const FATAL: u8 = 2;
const PROTOCOL_VERSION: u8 = 70;

/// What a sipx TLS listener answered a `ClientHello` with: the record type, and its first two
/// bytes of payload.
///
/// Two bytes is what an alert is, so this is the whole answer for a refusal and the start of a
/// `ServerHello` otherwise.
#[derive(Debug, PartialEq, Eq)]
struct Answer {
    content_type: u8,
    body: [u8; 2],
}

impl Answer {
    fn is_alert(&self, level: u8, description: u8) -> bool {
        self.content_type == ALERT && self.body == [level, description]
    }
}

/// What offering a version got: the answer, whether the listener then closed, and whether
/// anything reached the application behind it.
#[derive(Debug)]
struct Outcome {
    answer: Option<Answer>,
    closed: bool,
    /// Weak on purpose, and labelled so nobody reads more into it. A raw client sends no SIP
    /// message, so nothing would arrive whether the handshake was refused or accepted — this
    /// catches a refused connection being adopted into the pool anyway, and nothing else. The
    /// alert and `closed` carry the claim.
    reached_the_application: bool,
}

/// Offer this version to a sipx TLS listener and report what came back.
///
/// The listener is the real one — `bind` with `tls_server` set, as `sipx-call` configures it — so
/// this measures what a peer dialling a sipx endpoint gets, not what a bare acceptor would.
async fn offer(version: u16) -> Outcome {
    let ca = Ca::new();
    let (cert, key) = ca.issue(&[dns("localhost")], "localhost");
    let identity = Identity::from_pem(cert.as_bytes(), key.as_bytes()).expect("an identity");
    let server_tls = ServerTls::new(identity).expect("a server");

    let mut config = Config::new("127.0.0.1:0".parse().expect("valid"));
    config.tls_server = Some((server_tls, 0));
    let (endpoint, mut inbound) = bind(config).await.expect("binds");
    let tls_addr = endpoint.tls_addr().expect("a TLS port was bound");

    let mut stream = TcpStream::connect(tls_addr).await.expect("connects");
    stream
        .write_all(&client_hello(version))
        .await
        .expect("the hello goes out");

    // Five bytes is a record header; the payload's first two are read separately so that a short
    // answer is reported as "nothing" rather than hanging on the difference.
    let mut header = [0u8; 5];
    let answer = match tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut header))
        .await
        .expect("the listener answers or closes within five seconds")
    {
        Ok(_) => {
            let mut body = [0u8; 2];
            tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut body))
                .await
                .expect("no timeout")
                .expect("a record carries at least two bytes");
            Some(Answer {
                content_type: header[0],
                body,
            })
        }
        // The peer closed without a record. Refusal, but a mute one — distinguished from an
        // alert because the two are different operational experiences.
        Err(_) => None,
    };

    // And then does it close? `docs/specs/sip-tls.md` §3.1: a failure closes the connection — no
    // retry, no "continue anyway". This is the half that cannot pass vacuously, because an
    // accepted handshake leaves the socket open with the rest of the server's flight on it.
    let mut rest = [0u8; 1];
    let closed = matches!(
        tokio::time::timeout(Duration::from_secs(5), stream.read(&mut rest)).await,
        Ok(Ok(0))
    );

    Outcome {
        answer,
        closed,
        reached_the_application: inbound.try_recv().is_ok(),
    }
}

/// A `ClientHello` offering exactly one version, the way a client of that vintage would: in
/// `client_version`, with no `supported_versions` extension.
///
/// Everything else is held constant across the versions this file offers, which is what makes the
/// comparison mean anything: if the 1.0 offer were refused for a missing extension rather than for
/// its version, the 1.2 offer built by the same code would be refused too.
fn client_hello(version: u16) -> Vec<u8> {
    let mut extensions = Vec::new();
    // server_name (RFC 6066 §3), naming the host the fixture certificate is for.
    extensions.extend(extension(
        0x0000,
        &u16_prefixed(&[&[0x00u8][..], &u16_prefixed(b"localhost")].concat()),
    ));
    // supported_groups: x25519, secp256r1.
    extensions.extend(extension(0x000a, &u16_prefixed(&[0x00, 0x1d, 0x00, 0x17])));
    // ec_point_formats: uncompressed.
    extensions.extend(extension(0x000b, &u8_prefixed(&[0x00])));
    // signature_algorithms, which a server may demand before it looks at the version at all —
    // ecdsa_secp256r1_sha256, rsa_pss_rsae_sha256, rsa_pkcs1_sha256. The fixture CA issues
    // ECDSA P-256, so the first one is the one that matters.
    extensions.extend(extension(
        0x000d,
        &u16_prefixed(&[0x04, 0x03, 0x08, 0x04, 0x04, 0x01]),
    ));
    // Deliberately no `supported_versions` (RFC 8446 §4.2.1): it did not exist before 1.3, and a
    // hello carrying it would be negotiating from that extension instead of from
    // `client_version`, which is the field this test is about.

    let mut body = version.to_be_bytes().to_vec();
    // `random`. Fixed rather than generated: no handshake here gets far enough to use it, and a
    // fixed value keeps a failure reproducible.
    body.extend([0x43u8; 32]);
    body.extend(u8_prefixed(&[])); // no session to resume
    body.extend(u16_prefixed(&[
        0xc0, 0x2b, // ECDHE_ECDSA_WITH_AES_128_GCM_SHA256
        0xc0, 0x2f, // ECDHE_RSA_WITH_AES_128_GCM_SHA256
        0x13, 0x01, // TLS_AES_128_GCM_SHA256, for the 1.2-and-1.3 control
        0x13, 0x02, // TLS_AES_256_GCM_SHA384
    ]));
    body.extend(u8_prefixed(&[0x00])); // null compression, the only one still legal
    body.extend(u16_prefixed(&extensions));

    let handshake = [&[0x01u8][..], &u24_prefixed(&body)].concat();
    // The record layer carries the same version. A real client of this vintage sends its own
    // there, and rustls tolerates either — the negotiation reads `client_version`.
    [
        &[HANDSHAKE][..],
        &version.to_be_bytes(),
        &u16_prefixed(&handshake),
    ]
    .concat()
}

fn u8_prefixed(body: &[u8]) -> Vec<u8> {
    let len = u8::try_from(body.len()).expect("a byte-counted field fits in a byte");
    [&[len][..], body].concat()
}

fn u16_prefixed(body: &[u8]) -> Vec<u8> {
    let len = u16::try_from(body.len()).expect("a two-byte-counted field fits");
    [&len.to_be_bytes()[..], body].concat()
}

fn u24_prefixed(body: &[u8]) -> Vec<u8> {
    let len = u32::try_from(body.len()).expect("a handshake body fits in three bytes");
    [&len.to_be_bytes()[1..], body].concat()
}

fn extension(kind: u16, body: &[u8]) -> Vec<u8> {
    [&kind.to_be_bytes()[..], &u16_prefixed(body)].concat()
}

/// L9's sibling: 1.0 is refused, and refused *for its version*.
///
/// The alert description is asserted rather than only the failure, because "refused" on its own
/// would also be satisfied by a hello this test built wrong.
#[tokio::test]
async fn tls_1_0_is_refused_as_a_version() {
    let outcome = offer(TLS_1_0).await;
    let answer = outcome
        .answer
        .as_ref()
        .expect("the listener says why rather than going quiet");
    assert!(
        answer.is_alert(FATAL, PROTOCOL_VERSION),
        "a 1.0 offer must draw a fatal protocol_version alert, not {answer:?}"
    );
    assert!(
        outcome.closed,
        "a refused handshake closes the connection rather than leaving it usable"
    );
    assert!(
        !outcome.reached_the_application,
        "nothing may cross a connection that was refused"
    );
}

/// `docs/specs/sip-tls.md` §6, vector L9.
#[tokio::test]
async fn tls_1_1_is_refused_as_a_version() {
    let outcome = offer(TLS_1_1).await;
    let answer = outcome
        .answer
        .as_ref()
        .expect("the listener says why rather than going quiet");
    assert!(
        answer.is_alert(FATAL, PROTOCOL_VERSION),
        "a 1.1 offer must draw a fatal protocol_version alert, not {answer:?}"
    );
    assert!(
        outcome.closed,
        "a refused handshake closes the connection rather than leaving it usable"
    );
    assert!(
        !outcome.reached_the_application,
        "nothing may cross a connection that was refused"
    );
}

/// The control, and the reason the two tests above prove anything.
///
/// The same bytes with four of them changed are accepted — the two version bytes in the record
/// header and the two in `client_version`, and nothing else: the listener answers with a handshake
/// record rather than an alert. So the refusals above are about the version offered and not about
/// a `ClientHello` this file assembled wrong — which is the way a hand-built negative test usually
/// passes for the wrong reason.
#[tokio::test]
async fn the_same_hello_at_1_2_is_accepted() {
    let answer = offer(TLS_1_2).await.answer;
    let answer = answer.expect("the listener answers a 1.2 offer");
    assert_eq!(
        answer.content_type, HANDSHAKE,
        "1.2 is the floor and must be at it, not below: {answer:?}"
    );
    assert!(
        !answer.is_alert(FATAL, PROTOCOL_VERSION),
        "the version cannot be what a 1.2 offer is refused for"
    );
}

/// Where the property actually lives.
///
/// The refusals above are behaviour; this is the reason for it, and it belongs to a dependency.
/// The TLS library offers two versions and neither is below 1.2, so sipx has nothing to configure
/// downward and no code of its own to get wrong. Asserted here so that a backend which grows a
/// third, older version fails this rather than quietly widening what RFC 8996's registry row
/// claims — the row cites this file for exactly that reason.
#[test]
fn the_library_offers_nothing_below_the_floor() {
    use tokio_rustls::rustls::ALL_VERSIONS;
    use tokio_rustls::rustls::ProtocolVersion;

    let offered: Vec<ProtocolVersion> = ALL_VERSIONS.iter().map(|v| v.version).collect();
    assert_eq!(
        offered,
        vec![ProtocolVersion::TLSv1_3, ProtocolVersion::TLSv1_2],
        "the floor holds because the TLS library has no older version to select"
    );
}
