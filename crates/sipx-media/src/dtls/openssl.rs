//! The DTLS handshake itself, over the media socket.
//!
//! **Experimental** (`A-8`): behind the `dtls` feature, which no shipped binary enables, so this is
//! reachable from the library and from no application and nothing above it has constrained its shape
//! (`X-38`). [`super`] says the same of what it keys; this module says it on its own page because a
//! reader who lands here should not have to go up a level to find out (`A-8`'s rule).
//!
//! Everything RFC 5764 *decides* is in [`super`] and is compiled whatever the features say. This
//! module is only the record layer and the handshake, and it is behind the `dtls` feature because
//! it is where the C dependency lives.
//!
//! Why not a pure-Rust one: there is no DTLS implementation in Rust with comparable scrutiny, and
//! a hand-rolled handshake for a security-critical protocol is the kind of liability this project
//! declines elsewhere — the same reasoning that has SRTP's AES come from `RustCrypto` rather than
//! from here. OpenSSL is also where `use_srtp` (RFC 5764 §4.1.1) and the RFC 5705 exporter have
//! been exercised against every other implementation for a decade, which is what a keying
//! mechanism needs most.

use std::io::{Read, Write};
use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

use openssl::ssl::{Ssl, SslContext, SslMethod, SslOptions, SslStream, SslVerifyMode};

use super::{Handshake, Profile, Role};

/// Why a handshake could not be run.
#[derive(Debug, thiserror::Error)]
pub enum DtlsError {
    /// OpenSSL refused something.
    #[error("openssl: {0}")]
    Ssl(String),
    /// The socket failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// The peer proposed no protection profile sipx implements.
    #[error("no SRTP protection profile in common")]
    NoProfile,
}

impl From<openssl::error::ErrorStack> for DtlsError {
    fn from(error: openssl::error::ErrorStack) -> Self {
        Self::Ssl(error.to_string())
    }
}

/// A self-signed certificate to present on the media path, and its fingerprint.
///
/// RFC 5763 §5 wants a self-signed certificate here and says why the absence of a chain does not
/// matter: what authenticates the peer is not the certificate's issuer but the fingerprint that
/// arrived in the signalling. A certificate authority would authenticate a *name*, and there is no
/// name on a media path to authenticate.
pub struct Identity {
    certificate: openssl::x509::X509,
    key: openssl::pkey::PKey<openssl::pkey::Private>,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity").finish_non_exhaustive()
    }
}

impl Identity {
    /// Mint a fresh self-signed certificate.
    ///
    /// One per call is fine and one per process is fine; what must not happen is presenting a
    /// certificate whose fingerprint is not the one the SDP announced, so the two are produced
    /// together and [`Identity::fingerprint`] is the only way to get one.
    pub fn generate() -> Result<Self, DtlsError> {
        use openssl::asn1::Asn1Time;
        use openssl::bn::{BigNum, MsbOption};
        use openssl::ec::{EcGroup, EcKey};
        use openssl::hash::MessageDigest;
        use openssl::nid::Nid;
        use openssl::x509::{X509, X509NameBuilder};

        // P-256, because it is what every WebRTC endpoint negotiates and the point of DTLS-SRTP
        // here is to be callable by one.
        let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1)?;
        let key = openssl::pkey::PKey::from_ec_key(EcKey::generate(&group)?)?;

        let mut name = X509NameBuilder::new()?;
        // The name is not checked by anything — §5 authenticates the fingerprint, not the subject
        // — so it says what it is rather than pretending to be a host.
        name.append_entry_by_nid(Nid::COMMONNAME, "sipx DTLS-SRTP")?;
        let name = name.build();

        let mut builder = X509::builder()?;
        builder.set_version(2)?;
        let mut serial = BigNum::new()?;
        serial.rand(159, MsbOption::MAYBE_ZERO, false)?;
        let serial = serial.to_asn1_integer()?;
        builder.set_serial_number(&serial)?;
        builder.set_subject_name(&name)?;
        builder.set_issuer_name(&name)?;
        builder.set_pubkey(&key)?;
        let not_before = Asn1Time::days_from_now(0)?;
        builder.set_not_before(&not_before)?;
        // Thirty days. A media certificate outliving the call by a month is generous and still
        // bounded; an unbounded one is a key with no end of life.
        let not_after = Asn1Time::days_from_now(30)?;
        builder.set_not_after(&not_after)?;
        builder.sign(&key, MessageDigest::sha256())?;

        Ok(Self {
            certificate: builder.build(),
            key,
        })
    }

    /// The fingerprint to put in the SDP (RFC 8122).
    pub fn fingerprint(&self) -> Result<sipx_sdp::fingerprint::Fingerprint, DtlsError> {
        let der = self.certificate.to_der()?;
        Ok(sipx_sdp::fingerprint::Fingerprint::of(
            &der,
            sipx_sdp::fingerprint::HashFunc::Sha256,
        ))
    }
}

/// A UDP socket connected to one peer, presented to OpenSSL as a stream.
///
/// DTLS is datagram-oriented and OpenSSL wants something that reads and writes; a connected
/// `UdpSocket` is both, and connecting it is what makes the record layer see only this peer's
/// packets. It also means the kernel drops everything else — which is fine here because a media
/// port that is doing DTLS is doing it with the party the SDP named.
#[derive(Debug)]
struct Datagrams {
    socket: UdpSocket,
}

impl Read for Datagrams {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.socket.recv(buf)
    }
}

impl Write for Datagrams {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.socket.send(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A DTLS-SRTP handshake over a media socket.
pub struct Session {
    stream: Option<SslStream<Datagrams>>,
    pending: Option<Ssl>,
    socket: Option<UdpSocket>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("handshaken", &self.stream.is_some())
            .finish_non_exhaustive()
    }
}

impl Session {
    /// Prepare a handshake with `peer`, presenting `identity`.
    ///
    /// `timeout` bounds the handshake. DTLS retransmits its own flights, so without one a lost
    /// final flight leaves both ends waiting — and a media path that never keys is worse than one
    /// that fails, because the call is up and silent.
    pub fn new(
        socket: UdpSocket,
        peer: SocketAddr,
        identity: &Identity,
        timeout: Duration,
    ) -> Result<Self, DtlsError> {
        socket.connect(peer)?;
        socket.set_read_timeout(Some(timeout))?;
        socket.set_write_timeout(Some(timeout))?;

        let mut context = SslContext::builder(SslMethod::dtls())?;
        context.set_certificate(&identity.certificate)?;
        context.set_private_key(&identity.key)?;
        // RFC 5764 §4.1.1: the profiles this endpoint will accept, in preference order. Only the
        // one `sipx-rtp` can actually perform — offering more would be agreeing to a transform
        // this stack cannot apply.
        context.set_tlsext_use_srtp(Profile::Aes128CmHmacSha1_80.as_str())?;
        // The peer's certificate is *requested* and not validated by OpenSSL, because there is
        // nothing for it to validate against: RFC 5763 §5 expects a self-signed certificate, and
        // what authenticates it is the fingerprint from the SDP. `super::establish` performs that
        // check, and refuses the keys if it fails — so this is not verification being skipped, it
        // is verification happening somewhere OpenSSL cannot see.
        context.set_verify_callback(
            SslVerifyMode::PEER | SslVerifyMode::FAIL_IF_NO_PEER_CERT,
            |_valid, _store| true,
        );
        // DTLS 1.0 is long dead and 1.2 is what every peer speaks.
        context.set_options(SslOptions::NO_DTLSV1);

        Ok(Self {
            pending: Some(Ssl::new(&context.build())?),
            stream: None,
            socket: Some(socket),
        })
    }
}

impl Handshake for Session {
    type Error = DtlsError;

    fn run(&mut self, role: Role) -> Result<(), Self::Error> {
        let (Some(ssl), Some(socket)) = (self.pending.take(), self.socket.take()) else {
            // Already handshaken. Running twice would start a renegotiation nobody asked for.
            return Ok(());
        };
        let datagrams = Datagrams { socket };
        let mut stream = SslStream::new(ssl, datagrams)?;
        // The role is the negotiated `a=setup`, never a guess: a UA that connects when it agreed
        // to accept meets one coming the other way, and both time out.
        let outcome = match role {
            Role::Client => stream.connect(),
            Role::Server => stream.accept(),
        };
        outcome.map_err(|error| DtlsError::Ssl(error.to_string()))?;
        self.stream = Some(stream);
        Ok(())
    }

    fn peer_certificate(&self) -> Option<Vec<u8>> {
        self.stream
            .as_ref()?
            .ssl()
            .peer_certificate()?
            .to_der()
            .ok()
    }

    fn profile(&self) -> Option<Profile> {
        let name = self.stream.as_ref()?.ssl().selected_srtp_profile()?.name();
        (name == Profile::Aes128CmHmacSha1_80.as_str()).then_some(Profile::Aes128CmHmacSha1_80)
    }

    fn export(&self, len: usize) -> Result<Vec<u8>, Self::Error> {
        let stream = self.stream.as_ref().ok_or(DtlsError::NoProfile)?;
        let mut out = vec![0u8; len];
        // RFC 5705's exporter with RFC 5764 §4.2's label and **no context**. A zero-length context
        // and an absent one derive different keys, and §4.2 specifies absent — passing an empty
        // slice here is the mistake that produces a handshake both ends complete and no packet
        // either can decrypt.
        stream
            .ssl()
            .export_keying_material(&mut out, super::EXPORTER_LABEL, None)
            .map_err(|error| DtlsError::Ssl(error.to_string()))?;
        Ok(out)
    }
}
