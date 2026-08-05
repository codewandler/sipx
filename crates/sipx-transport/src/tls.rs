//! SIP over TLS (RFC 3261 §26, RFC 5922).
//!
//! A TLS connection differs from a TCP one in its bytes, not in its transaction handling — so
//! this crate reuses [`crate::tcp`]'s framing and pool wholesale and adds only the handshake
//! and the verification around it.
//!
//! The verification is the point, and `docs/specs/sip-tls.md` settles what it is. Two decisions
//! from there govern this file:
//!
//! **There is no way to turn it off.** No `insecure` flag, no `danger_accept_invalid_certs`.
//! Code that needs to trust a fixture CA adds that CA as a trust anchor — a different operation
//! with a different shape, saying *what* to trust rather than *that anything goes*. Every stack
//! that ships the other kind of flag eventually finds it in production.
//!
//! **The name checked is the one sipx set out to reach**, not the name a SRV record led to.
//! Checking the resolved name would let whoever can influence DNS choose which certificate is
//! acceptable, and the verification becomes decorative.
//!
//! **The version floor is the library's, not ours** (§3.5, RFC 8996). Both configurations below
//! are built from `rustls`'s default version set, and neither this file nor anything above it
//! names a version — so 1.0 and 1.1 are excluded because the library has nothing older than 1.2
//! to select, not because sipx refuses them. That makes the floor a *dependency* property: a
//! backend that still spoke 1.0 would move it without a line changing here, which is why RFC
//! 8996's registry row cites `tests/tls_versions.rs` — the refusal observed on the wire, plus the
//! version set asserted — rather than the sentence in the spec.

use std::sync::Arc;

use rustls_pki_types::pem::PemObject as _;
use rustls_pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use tokio_rustls::rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio_rustls::{TlsAcceptor, TlsConnector};

/// What can go wrong establishing TLS.
///
/// The variants are separate because expired, wrong-host and unknown-issuer are three different
/// operational problems with three different fixes. Collapsing them into "handshake failed"
/// costs an engineer an afternoon.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TlsError {
    /// The name to verify against is not a valid DNS name.
    #[error("{0} is not a name a certificate can be checked against")]
    UnusableName(String),
    /// A certificate or key could not be read.
    #[error("reading {what}: {detail}")]
    Material {
        /// Which file or blob.
        what: String,
        /// What was wrong with it.
        detail: String,
    },
    /// The configuration itself is invalid.
    #[error("tls configuration: {0}")]
    Config(String),
    /// The handshake failed — including every verification failure, which rustls reports as an
    /// alert with its reason attached.
    #[error("tls handshake with {peer}: {detail}")]
    Handshake {
        /// Who we were talking to.
        peer: String,
        /// What went wrong, as reported by the TLS library.
        detail: String,
    },
}

/// How sipx behaves as a TLS client.
#[derive(Clone)]
pub struct ClientTls {
    config: Arc<ClientConfig>,
}

impl std::fmt::Debug for ClientTls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The config holds keys; printing it would put them in a log.
        f.write_str("ClientTls { .. }")
    }
}

/// Which certificates to trust.
#[derive(Debug, Clone, Default)]
pub struct TrustAnchors {
    /// Additional roots, on top of or instead of the system's.
    extra: Vec<CertificateDer<'static>>,
    /// Whether to include the platform's own roots.
    system: bool,
}

impl TrustAnchors {
    /// Whatever the platform trusts.
    ///
    /// The *platform's* store, not a copy of one vendor's list compiled in. The difference
    /// matters twice: an operator who adds a corporate CA expects sipx to honour it, and a
    /// root that is distrusted after a compromise stops being trusted when the OS says so
    /// rather than when someone remembers to bump a dependency.
    #[must_use]
    pub fn system() -> Self {
        Self {
            extra: Vec::new(),
            system: true,
        }
    }

    /// Trust only what is added here.
    ///
    /// This is what a test uses. Note the shape: it names the CA to trust rather than
    /// disabling the check, so a mistake produces a *failed* handshake rather than a silently
    /// accepted one.
    #[must_use]
    pub fn only() -> Self {
        Self {
            extra: Vec::new(),
            system: false,
        }
    }

    /// Add a PEM-encoded certificate as a trust anchor.
    pub fn add_pem(&mut self, pem: &[u8]) -> Result<(), TlsError> {
        let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(pem)
            .collect::<Result<_, _>>()
            .map_err(|error| TlsError::Material {
                what: "trust anchor".to_owned(),
                detail: error.to_string(),
            })?;
        if certs.is_empty() {
            return Err(TlsError::Material {
                what: "trust anchor".to_owned(),
                detail: "no certificate found in the PEM data".to_owned(),
            });
        }
        self.extra.extend(certs);
        Ok(())
    }

    fn store(&self) -> Result<RootCertStore, TlsError> {
        let mut store = RootCertStore::empty();
        if self.system {
            let loaded = rustls_native_certs::load_native_certs();
            for error in &loaded.errors {
                // Reported rather than swallowed: a partially loaded store fails handshakes
                // that ought to succeed, and the reason is otherwise invisible.
                tracing::warn!(%error, "could not read part of the platform trust store");
            }
            for cert in loaded.certs {
                // A platform store may hold a certificate rustls will not parse. Skipping it
                // is right — one unusable root must not cost the other two hundred.
                if let Err(error) = store.add(cert) {
                    tracing::debug!(%error, "skipping an unusable root from the platform store");
                }
            }
        }
        for cert in &self.extra {
            store
                .add(cert.clone())
                .map_err(|error| TlsError::Config(error.to_string()))?;
        }
        if store.is_empty() {
            return Err(TlsError::Config(
                "no trust anchors: every certificate would be refused".to_owned(),
            ));
        }
        Ok(store)
    }
}

impl ClientTls {
    /// A client that verifies against these anchors.
    pub fn new(anchors: &TrustAnchors) -> Result<Self, TlsError> {
        Self::with_identity(anchors, None)
    }

    /// A client that also presents a certificate of its own (mutual TLS).
    ///
    /// When a server asks for one and none is configured, the handshake proceeds without it and
    /// the server decides. sipx does not pre-emptively fail, because plenty of servers ask
    /// optionally.
    pub fn with_identity(
        anchors: &TrustAnchors,
        identity: Option<Identity>,
    ) -> Result<Self, TlsError> {
        let roots = anchors.store()?;
        let builder = ClientConfig::builder().with_root_certificates(roots);

        let config = match identity {
            Some(identity) => builder
                .with_client_auth_cert(identity.chain, identity.key)
                .map_err(|error| TlsError::Config(error.to_string()))?,
            None => builder.with_no_client_auth(),
        };

        Ok(Self {
            config: Arc::new(config),
        })
    }

    /// A connector for one peer.
    #[must_use]
    pub fn connector(&self) -> TlsConnector {
        TlsConnector::from(Arc::clone(&self.config))
    }

    /// Reuse this exact trust and identity policy for a QUIC connection.
    #[cfg(feature = "quic")]
    pub(crate) fn quic_config(&self) -> Result<quinn::ClientConfig, TlsError> {
        let config = self.quic_rustls_config();
        let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(config)
            .map_err(|error| TlsError::Config(error.to_string()))?;
        Ok(quinn::ClientConfig::new(Arc::new(crypto)))
    }

    #[cfg(feature = "quic")]
    fn quic_rustls_config(&self) -> ClientConfig {
        let mut config = (*self.config).clone();
        config.alpn_protocols = vec![b"sip/2".to_vec()];
        config.enable_early_data = false;
        config
    }
}

/// A certificate and key sipx presents.
pub struct Identity {
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The key is deliberately opaque in every diagnostic, including a refused reload.
        f.write_str("Identity { .. }")
    }
}

impl Identity {
    /// Read a certificate chain and key from PEM.
    pub fn from_pem(cert_pem: &[u8], key_pem: &[u8]) -> Result<Self, TlsError> {
        let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert_pem)
            .collect::<Result<_, _>>()
            .map_err(|error| TlsError::Material {
                what: "certificate".to_owned(),
                detail: error.to_string(),
            })?;
        if chain.is_empty() {
            return Err(TlsError::Material {
                what: "certificate".to_owned(),
                detail: "no certificate found in the PEM data".to_owned(),
            });
        }

        let key = PrivateKeyDer::from_pem_slice(key_pem).map_err(|error| TlsError::Material {
            what: "private key".to_owned(),
            detail: error.to_string(),
        })?;

        Ok(Self { chain, key })
    }

    /// Prove that every supplied issuer certificate belongs to the server chain.
    ///
    /// A server normally omits its trust root, so the last supplied certificate is the path's
    /// provisional anchor. With a leaf alone there is no issuer material to validate here; the
    /// peer still validates that leaf against its own anchors during the handshake. With two or
    /// more certificates, every certificate after the leaf must be consumed, in the supplied
    /// order, by one valid server-authentication path. That rejects both malformed certificates
    /// and harmless-looking unrelated extras before a listener can publish them.
    fn validate_server_chain(&self) -> Result<(), TlsError> {
        let Some((leaf, issuers)) = self.chain.split_first() else {
            return Err(TlsError::Config(
                "server certificate chain has no leaf".to_owned(),
            ));
        };
        let end_entity = webpki::EndEntityCert::try_from(leaf).map_err(|error| {
            TlsError::Config(format!("invalid server certificate leaf: {error}"))
        })?;
        let Some((anchor_certificate, intermediates)) = issuers.split_last() else {
            return Ok(());
        };
        let anchor = webpki::anchor_from_trusted_cert(anchor_certificate).map_err(|error| {
            TlsError::Config(format!("invalid server certificate chain anchor: {error}"))
        })?;
        let provider = tokio_rustls::rustls::crypto::ring::default_provider();
        let anchors = [anchor];
        let verified = end_entity
            .verify_for_usage(
                provider.signature_verification_algorithms.all,
                &anchors,
                intermediates,
                UnixTime::now(),
                webpki::KeyUsage::server_auth(),
                None,
                None,
            )
            .map_err(|error| {
                TlsError::Config(format!("invalid server certificate chain: {error}"))
            })?;
        let supplied_in_order = verified
            .intermediate_certificates()
            .map(webpki::Cert::der)
            .eq(intermediates.iter().cloned());
        if !supplied_in_order {
            return Err(TlsError::Config(
                "server certificate chain contains an unrelated or out-of-order certificate"
                    .to_owned(),
            ));
        }
        Ok(())
    }
}

/// How sipx behaves as a TLS server.
#[derive(Clone)]
pub struct ServerTls {
    config: Arc<ServerConfig>,
}

impl std::fmt::Debug for ServerTls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ServerTls { .. }")
    }
}

impl ServerTls {
    /// A server presenting this identity, not asking for a client certificate.
    pub fn new(identity: Identity) -> Result<Self, TlsError> {
        identity.validate_server_chain()?;
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(identity.chain, identity.key)
            .map_err(|error| TlsError::Config(error.to_string()))?;
        Ok(Self {
            config: Arc::new(config),
        })
    }

    /// An acceptor for incoming connections.
    #[must_use]
    pub fn acceptor(&self) -> TlsAcceptor {
        TlsAcceptor::from(Arc::clone(&self.config))
    }

    /// Reuse this exact identity policy for the QUIC handshake.
    #[cfg(feature = "quic")]
    pub(crate) fn quic_config(&self) -> Result<quinn::ServerConfig, TlsError> {
        let config = self.quic_rustls_config();
        let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(config)
            .map_err(|error| TlsError::Config(error.to_string()))?;
        Ok(quinn::ServerConfig::with_crypto(Arc::new(crypto)))
    }

    #[cfg(feature = "quic")]
    fn quic_rustls_config(&self) -> ServerConfig {
        let mut config = (*self.config).clone();
        config.alpn_protocols = vec![b"sip/2".to_vec()];
        config.max_early_data_size = 0;
        config
    }
}

/// The name a certificate is checked against.
///
/// **The host from the URI sipx set out to reach**, not the name resolution produced. If the
/// resolved name were used, anyone who can influence DNS would choose which certificate is
/// acceptable — the handshake would still succeed, the check would still appear to run, and it
/// would mean nothing.
pub fn verification_name(uri_host: &str) -> Result<ServerName<'static>, TlsError> {
    ServerName::try_from(uri_host.to_owned())
        .map_err(|_| TlsError::UnusableName(uri_host.to_owned()))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn a_hostname_is_a_usable_verification_name() {
        assert!(verification_name("sip.example.com").is_ok());
        assert!(verification_name("example.com").is_ok());
    }

    /// An IP address is a usable server name in TLS, but a SIP URI naming one has no domain
    /// identity to check — the caller has to decide what that means rather than have this
    /// function guess.
    #[test]
    fn an_address_is_accepted_as_a_name() {
        assert!(verification_name("192.0.2.1").is_ok());
    }

    #[test]
    fn something_that_is_not_a_name_is_refused_by_name() {
        let error = verification_name("not a hostname!").expect_err("refused");
        assert!(error.to_string().contains("not a hostname!"), "{error}");
    }

    /// Trusting nothing is a configuration error rather than a silent refusal of everything:
    /// the second would look like a network problem at every call site.
    #[test]
    fn a_client_with_no_anchors_is_refused_at_construction() {
        let error = ClientTls::new(&TrustAnchors::only()).expect_err("refused");
        assert!(error.to_string().contains("no trust anchors"), "{error}");
    }

    #[test]
    fn the_system_anchors_are_enough_to_build_a_client() {
        assert!(ClientTls::new(&TrustAnchors::system()).is_ok());
    }

    #[test]
    fn pem_that_holds_no_certificate_is_refused_by_name() {
        let mut anchors = TrustAnchors::only();
        let error = anchors.add_pem(b"not a certificate").expect_err("refused");
        assert!(error.to_string().contains("no certificate"), "{error}");
    }

    #[test]
    fn an_identity_needs_both_halves() {
        let error = Identity::from_pem(b"", b"").expect_err("refused");
        assert!(error.to_string().contains("certificate"), "{error}");
    }

    /// The configuration holds private keys. A `Debug` that printed them would put them in
    /// whatever log the caller writes.
    #[test]
    fn debug_output_does_not_leak_key_material() {
        let client = ClientTls::new(&TrustAnchors::system()).expect("builds");
        let printed = format!("{client:?}");
        assert_eq!(printed, "ClientTls { .. }");

        let ca = sipx_testkit::certs::Ca::new();
        let (certificate, key) = ca.issue_for("localhost");
        let identity =
            Identity::from_pem(certificate.as_bytes(), key.as_bytes()).expect("identity");
        assert_eq!(format!("{identity:?}"), "Identity { .. }");
    }

    #[cfg(feature = "quic")]
    #[test]
    fn quic_requires_sip2_and_refuses_early_data_in_both_directions() {
        let client = ClientTls::new(&TrustAnchors::system()).expect("client");
        let client = client.quic_rustls_config();
        assert_eq!(client.alpn_protocols, [b"sip/2".to_vec()]);
        assert!(!client.enable_early_data);

        let ca = sipx_testkit::certs::Ca::new();
        let (certificate, key) = ca.issue_for("localhost");
        let identity =
            Identity::from_pem(certificate.as_bytes(), key.as_bytes()).expect("identity");
        let server = ServerTls::new(identity)
            .expect("server")
            .quic_rustls_config();
        assert_eq!(server.alpn_protocols, [b"sip/2".to_vec()]);
        assert_eq!(server.max_early_data_size, 0);
    }

    /// Q13: even a client holding early-data-capable resumption state cannot deliver a request
    /// before sipx's server handshake completes; its retry is exposed only as a 1-RTT stream.
    #[cfg(feature = "quic")]
    #[tokio::test]
    async fn a_resumed_client_cannot_deliver_early_data_to_a_sipx_server() {
        let ca = sipx_testkit::certs::Ca::new();
        let (certificate, key) = ca.issue_for("localhost");
        let identity =
            Identity::from_pem(certificate.as_bytes(), key.as_bytes()).expect("identity");
        let server_policy = ServerTls::new(identity).expect("server policy");

        // The first handshake deliberately issues an early-data-capable ticket. The second
        // configuration comes through sipx's production conversion and shares the underlying
        // rustls ticket machinery through the cloned policy.
        let mut permissive = server_policy.quic_rustls_config();
        permissive.max_early_data_size = u32::MAX;
        let permissive = quinn::crypto::rustls::QuicServerConfig::try_from(permissive)
            .map(|crypto| quinn::ServerConfig::with_crypto(Arc::new(crypto)))
            .expect("permissive ticket server");
        let rejecting = server_policy.quic_config().expect("sipx QUIC server");
        let server =
            quinn::Endpoint::server(permissive, "127.0.0.1:0".parse().expect("server address"))
                .expect("server endpoint");
        let server_addr = server.local_addr().expect("server address");

        let mut anchors = TrustAnchors::only();
        anchors
            .add_pem(ca.pem().as_bytes())
            .expect("test authority");
        let mut client_tls = ClientTls::new(&anchors)
            .expect("client policy")
            .quic_rustls_config();
        client_tls.enable_early_data = true;
        let client_crypto = quinn::crypto::rustls::QuicClientConfig::try_from(client_tls)
            .expect("early-data client");
        let mut client_config = quinn::ClientConfig::new(Arc::new(client_crypto));
        client_config.transport_config(crate::quic::transport_config());
        let mut client = quinn::Endpoint::client("127.0.0.1:0".parse().expect("client address"))
            .expect("client endpoint");
        client.set_default_client_config(client_config);

        let (ready, configured) = tokio::sync::oneshot::channel();
        let server_task = tokio::spawn(async move {
            let first = server
                .accept()
                .await
                .expect("first connection")
                .await
                .expect("first handshake");
            let (mut marker, _unused) = first.open_bi().await.expect("1-RTT marker stream");
            marker.write_all(b"ready").await.expect("1-RTT marker");
            marker.finish().expect("1-RTT marker finishes");

            server.set_server_config(Some(rejecting));
            ready.send(()).expect("client waits for configuration");
            let second = server
                .accept()
                .await
                .expect("resumed connection")
                .await
                .expect("resumption handshake");
            let (_reply, mut request) = second.accept_bi().await.expect("request stream");
            let was_early = request.is_0rtt();
            let bytes = request.read_to_end(1024).await.expect("request bytes");
            (was_early, bytes)
        });

        let first = client
            .connect(server_addr, "localhost")
            .expect("first connect starts")
            .await
            .expect("first connect");
        let (_unused, mut marker) = first.accept_bi().await.expect("server marker");
        assert_eq!(
            marker.read_to_end(16).await.expect("marker bytes"),
            b"ready"
        );
        drop(first);
        configured.await.expect("rejecting server installed");

        let (resumed, accepted) = client
            .connect(server_addr, "localhost")
            .expect("resumption starts")
            .into_0rtt()
            .expect("client has early-data keys");
        let (mut request, _reply) = resumed.open_bi().await.expect("early request stream");
        request
            .write_all(b"SIP request attempted as early data")
            .await
            .expect("early write is queued");
        request.finish().expect("request finishes");
        assert!(!accepted.await, "sipx accepted replayable early data");
        let (mut request, _reply) = resumed.open_bi().await.expect("1-RTT request stream");
        request
            .write_all(b"SIP request attempted as early data")
            .await
            .expect("1-RTT retry writes");
        request.finish().expect("1-RTT retry finishes");
        let (was_early, bytes) = server_task.await.expect("server task");
        assert!(!was_early, "the server exposed a 0-RTT request stream");
        assert_eq!(bytes, b"SIP request attempted as early data");
    }
}
