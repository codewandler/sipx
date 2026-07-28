//! DTLS-SRTP end to end (RFC 5763 / 5764 / 8122).
//!
//! Two real sockets, two real self-signed certificates, a real handshake, and the keys it derives
//! used to protect and unprotect a real RTP packet. Nothing is stubbed here — the stubs live in the
//! unit tests, and what they cannot prove is that OpenSSL's exporter, sipx's key split and sipx's
//! SRTP transform all agree with each other.

#![cfg(feature = "dtls")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::UdpSocket;
use std::time::Duration;

use sipx_media::dtls::openssl::{Identity, Session};
use sipx_media::dtls::{self, Role};
use sipx_sdp::fingerprint::{Fingerprint, HashFunc};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Two connected sockets on loopback.
fn pair() -> (UdpSocket, UdpSocket) {
    let a = UdpSocket::bind("127.0.0.1:0").expect("binds");
    let b = UdpSocket::bind("127.0.0.1:0").expect("binds");
    (a, b)
}

/// Run both halves of a handshake concurrently and return what each derived.
///
/// Concurrently because a DTLS handshake is a conversation: running the client to completion first
/// would deadlock on the very first flight.
fn handshake(
    answerer_fingerprint: Option<Fingerprint>,
    offerer_fingerprint: Option<Fingerprint>,
) -> (
    Result<dtls::Keys, dtls::Error>,
    Result<dtls::Keys, dtls::Error>,
) {
    let (client_socket, server_socket) = pair();
    let client_addr = client_socket.local_addr().expect("has an address");
    let server_addr = server_socket.local_addr().expect("has an address");

    let client_identity = Identity::generate().expect("a certificate");
    let server_identity = Identity::generate().expect("a certificate");

    // Each side checks the *other's* certificate against the fingerprint from its SDP.
    let client_checks = answerer_fingerprint
        .unwrap_or_else(|| server_identity.fingerprint().expect("a fingerprint"));
    let server_checks = offerer_fingerprint
        .unwrap_or_else(|| client_identity.fingerprint().expect("a fingerprint"));

    let server = std::thread::spawn(move || {
        let mut session = Session::new(
            server_socket,
            client_addr,
            &server_identity,
            HANDSHAKE_TIMEOUT,
        )
        .expect("a session");
        dtls::establish(&mut session, Role::Server, Some(&server_checks))
    });

    let mut session = Session::new(
        client_socket,
        server_addr,
        &client_identity,
        HANDSHAKE_TIMEOUT,
    )
    .expect("a session");
    let client = dtls::establish(&mut session, Role::Client, Some(&client_checks));

    (client, server.join().expect("the server thread finishes"))
}

/// The whole mechanism, proven by a packet crossing it.
///
/// Two endpoints that have never exchanged a key derive SRTP contexts that interoperate. The
/// signalling in this test carries only fingerprints — as it does in a real call — so nothing that
/// could read it would have learned anything usable.
#[test]
fn two_endpoints_key_srtp_by_handshaking_on_the_media_path() {
    let (client, server) = handshake(None, None);
    let client = client.expect("the client keys");
    let server = server.expect("the server keys");

    // An RTP packet: version 2, PCMU, one sequence number, a payload.
    let packet = [
        0x80, 0x00, 0x00, 0x2a, 0x00, 0x00, 0x05, 0x00, 0xca, 0xfe, 0xba, 0xbe, 0x11, 0x22, 0x33,
        0x44,
    ];

    let mut sending = client.outbound;
    let mut receiving = server.inbound;
    let protected = sending.protect(&packet).expect("protects");
    assert_ne!(
        protected.get(12..16),
        packet.get(12..16),
        "the payload should not be in the clear"
    );
    assert_eq!(
        receiving.unprotect(&protected).expect(
            "the server must be able to unprotect what the client protected; if this fails, the \
             exporter, the key split or the SRTP transform disagree"
        ),
        packet
    );

    // And back the other way, which a one-sided key split would leave broken.
    let mut sending = server.outbound;
    let mut receiving = client.inbound;
    let protected = sending.protect(&packet).expect("protects");
    assert_eq!(receiving.unprotect(&protected).expect("unprotects"), packet);
}

/// The story's failing-first test.
///
/// RFC 8122 §6.2: an endpoint whose peer's certificate "does not match the original fingerprint"
/// MUST "terminate the media connection with a `bad_certificate` error". A substituting intermediary
/// can complete a DTLS handshake perfectly well — what it cannot do is present the certificate the
/// signalling named, and this is the check that notices.
#[test]
fn a_mismatched_fingerprint_stops_the_media() {
    // The client is told to expect a certificate the server does not have.
    let wrong = Fingerprint::of(b"a certificate nobody on this path holds", HashFunc::Sha256);
    let (client, _server) = handshake(Some(wrong), None);

    assert!(
        matches!(client, Err(dtls::Error::FingerprintMismatch)),
        "a certificate that does not match the SDP must stop the media, not key it: {client:?}"
    );
}

/// A peer whose SDP carried no fingerprint at all is refused before anything is exchanged.
#[test]
fn a_peer_with_no_fingerprint_gets_no_media() {
    let (client_socket, server_socket) = pair();
    let server_addr = server_socket.local_addr().expect("has an address");
    let identity = Identity::generate().expect("a certificate");
    let mut session =
        Session::new(client_socket, server_addr, &identity, HANDSHAKE_TIMEOUT).expect("a session");

    let outcome = dtls::establish(&mut session, Role::Client, None);
    assert!(
        matches!(outcome, Err(dtls::Error::NoFingerprint)),
        "an unverifiable peer must be refused: {outcome:?}"
    );
}

/// The certificate sipx presents is the one its SDP announces. If these can drift apart, every
/// peer that does the check correctly rejects sipx — and the failure looks like a DTLS bug.
#[test]
fn the_fingerprint_in_the_sdp_is_of_the_certificate_that_is_presented() {
    let identity = Identity::generate().expect("a certificate");
    let announced = identity.fingerprint().expect("a fingerprint");

    let (client_socket, server_socket) = pair();
    let client_addr = client_socket.local_addr().expect("has an address");
    let server_addr = server_socket.local_addr().expect("has an address");
    let peer_identity = Identity::generate().expect("a certificate");
    let peer_fingerprint = peer_identity.fingerprint().expect("a fingerprint");

    let server = std::thread::spawn(move || {
        let mut session = Session::new(server_socket, client_addr, &identity, HANDSHAKE_TIMEOUT)
            .expect("a session");
        dtls::establish(&mut session, Role::Server, Some(&peer_fingerprint)).map(|_| ())
    });

    let mut session = Session::new(
        client_socket,
        server_addr,
        &peer_identity,
        HANDSHAKE_TIMEOUT,
    )
    .expect("a session");
    // The client checks the server against the fingerprint the server *announced*. It matching is
    // the assertion.
    dtls::establish(&mut session, Role::Client, Some(&announced))
        .expect("the announced fingerprint must be of the certificate actually presented");
    server.join().expect("the server finishes").expect("keys");
}

/// Certificates are per-identity, not shared. Two `Identity::generate` calls that produced the
/// same certificate would make every fingerprint check pass by accident.
#[test]
fn each_identity_is_its_own_certificate() {
    let one = Identity::generate().expect("a certificate");
    let two = Identity::generate().expect("a certificate");
    assert_ne!(
        one.fingerprint().expect("a fingerprint"),
        two.fingerprint().expect("a fingerprint")
    );
}
