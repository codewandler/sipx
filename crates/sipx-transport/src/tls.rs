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

use std::sync::Arc;

use rustls_pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use tokio_rustls::rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio_rustls::{TlsAcceptor, TlsConnector};

/// What can go wrong establishing TLS.
///
/// The variants are separate because expired, wrong-host and unknown-issuer are three different
/// operational problems with three different fixes. Collapsing them into "handshake failed"
/// costs an engineer an afternoon.
#[derive(Debug, thiserror::Error)]
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
    /// The usual set: whatever the platform trusts.
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
        let mut reader = std::io::BufReader::new(pem);
        let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut reader)
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
            store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
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
}

/// A certificate and key sipx presents.
#[derive(Debug)]
pub struct Identity {
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
}

impl Identity {
    /// Read a certificate chain and key from PEM.
    pub fn from_pem(cert_pem: &[u8], key_pem: &[u8]) -> Result<Self, TlsError> {
        let mut reader = std::io::BufReader::new(cert_pem);
        let chain: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut reader)
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

        let mut reader = std::io::BufReader::new(key_pem);
        let key = rustls_pemfile::private_key(&mut reader)
            .map_err(|error| TlsError::Material {
                what: "private key".to_owned(),
                detail: error.to_string(),
            })?
            .ok_or_else(|| TlsError::Material {
                what: "private key".to_owned(),
                detail: "no key found in the PEM data".to_owned(),
            })?;

        Ok(Self { chain, key })
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
    }
}
