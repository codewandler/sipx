//! Signalling transport policy shared by every command that opens an endpoint.
//!
//! The command layer decides what the user asked for before any socket is opened, then hands the
//! existing transport types the trust anchors, identity and verification name. Keeping that policy
//! here prevents `dial`, `register` and `answer` from acquiring three subtly different downgrade
//! rules.

use std::net::SocketAddr;

use sipx_transport::tls::{ClientTls, Identity, ServerTls, TrustAnchors};
use sipx_transport::{Config, Handle, Target, TransportKind};

use crate::cli::{SignallingOptions, TransportChoice};
use crate::output::Report;

/// A validated command-line transport selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Selection {
    kind: TransportKind,
    /// Only the new `--transport` surface changes result records. The legacy default and `--tcp`
    /// keep their existing byte-for-byte output contract.
    report: bool,
}

impl Selection {
    /// Resolve `--transport`, retaining `--tcp` as the compatible alias it already was.
    pub(crate) fn from_options(
        options: &SignallingOptions,
        secure_uri: bool,
    ) -> Result<Self, String> {
        let requested = options.transport.transport;

        let kind = match requested {
            Some(TransportChoice::Tcp) => TransportKind::Tcp,
            Some(TransportChoice::Tls) => TransportKind::Tls,
            Some(TransportChoice::Ws) => TransportKind::Ws,
            Some(TransportChoice::Wss) => TransportKind::Wss,
            None if options.transport.tcp => TransportKind::Tcp,
            Some(TransportChoice::Udp) | None => TransportKind::Udp,
        };
        if secure_uri && !kind.is_secure() {
            return Err(format!(
                "a sips: URI requires --transport tls or --transport wss; {} is cleartext and no downgrade is permitted",
                name(kind)
            ));
        }

        let has_tls_option = options.peer.tls_server_name.is_some()
            || options.peer.tls_ca.is_some()
            || options.identity.tls_cert.is_some()
            || options.identity.tls_key.is_some();
        if has_tls_option && !kind.is_secure() {
            return Err(format!(
                "TLS identity and trust options require --transport tls or --transport wss, not {}",
                name(kind)
            ));
        }
        identity(options, false)?;

        Ok(Self {
            kind,
            report: requested.is_some(),
        })
    }

    /// The selected transport kind.
    #[must_use]
    pub(crate) fn kind(self) -> TransportKind {
        self.kind
    }

    /// Whether an inbound request belongs to this command's listener contract.
    ///
    /// The historical no-flag answerer listened on both UDP and TCP, so its implicit UDP default
    /// keeps accepting either. An explicit `--transport`, and the legacy `--tcp` alias, select one.
    #[must_use]
    pub(crate) fn accepts(self, incoming: TransportKind) -> bool {
        (!self.report && self.kind == TransportKind::Udp) || incoming == self.kind
    }

    /// Add transport facts to a terminal result produced through the new selection surface.
    #[must_use]
    pub(crate) fn report(self, report: Report, negotiated: TransportKind) -> Report {
        if self.report {
            report
                .text("requested_transport", name(self.kind))
                .text("negotiated_transport", name(negotiated))
        } else {
            report
        }
    }

    /// Add the requested transport to a pre-call listener announcement.
    #[must_use]
    pub(crate) fn requested_report(self, report: Report) -> Report {
        if self.report {
            report.text("requested_transport", name(self.kind))
        } else {
            report
        }
    }

    /// Build an outbound target and carry the name that TLS/WSS must verify.
    pub(crate) fn target(
        self,
        options: &SignallingOptions,
        addr: SocketAddr,
        default_server_name: &str,
    ) -> Result<Target, String> {
        let target = Target::new(addr, self.kind);
        if self.kind.is_secure() {
            let verify_as = options
                .peer
                .tls_server_name
                .as_deref()
                .unwrap_or(default_server_name);
            sipx_transport::tls::verification_name(verify_as)
                .map_err(|error| format!("--tls-server-name: {error}"))?;
            Ok(target.verifying(verify_as))
        } else {
            Ok(target)
        }
    }

    /// Configure verification and optional mutual-TLS identity for outbound connections.
    pub(crate) fn configure_client(
        self,
        options: &SignallingOptions,
        config: &mut Config,
    ) -> Result<(), String> {
        if !self.kind.is_secure() {
            return Ok(());
        }

        let mut anchors = TrustAnchors::system();
        if let Some(path) = options.peer.tls_ca.as_deref() {
            let pem = read(path, "trust roots")?;
            anchors
                .add_pem(&pem)
                .map_err(|error| format!("--tls-ca {path}: {error}"))?;
        }
        let client_identity = identity(options, false)?;
        config.tls_client = Some(
            ClientTls::with_identity(&anchors, client_identity)
                .map_err(|error| format!("TLS client configuration: {error}"))?,
        );
        Ok(())
    }

    /// Configure the one listener `answer --transport` promises.
    pub(crate) fn configure_listener(
        self,
        options: &SignallingOptions,
        config: &mut Config,
    ) -> Result<(), String> {
        if options.peer.tls_server_name.is_some() || options.peer.tls_ca.is_some() {
            return Err(
                "--tls-server-name and --tls-ca configure an outbound TLS peer and are not valid for answer"
                    .to_owned(),
            );
        }
        if self.report {
            config.cleartext = match self.kind {
                TransportKind::Udp => sipx_transport::CleartextTransports::Udp,
                TransportKind::Tcp => sipx_transport::CleartextTransports::Tcp,
                TransportKind::Tls
                | TransportKind::Ws
                | TransportKind::Wss
                | TransportKind::Quic => sipx_transport::CleartextTransports::None,
            };
        }
        match self.kind {
            TransportKind::Udp | TransportKind::Tcp => Ok(()),
            TransportKind::Tls => {
                let server = ServerTls::new(
                    identity(options, true)?
                        .ok_or_else(|| "TLS identity is required".to_owned())?,
                )
                .map_err(|error| format!("TLS server configuration: {error}"))?;
                config.tls_server = Some((server, config.bind.port()));
                Ok(())
            }
            TransportKind::Ws => {
                config.ws_server = Some(config.bind.port());
                Ok(())
            }
            TransportKind::Wss => {
                let server = ServerTls::new(
                    identity(options, true)?
                        .ok_or_else(|| "TLS identity is required".to_owned())?,
                )
                .map_err(|error| format!("TLS server configuration: {error}"))?;
                config.wss_server = Some((server, config.bind.port()));
                Ok(())
            }
            TransportKind::Quic => Err(
                "the command-line QUIC listener is not wired yet; use the sipx-transport API"
                    .to_owned(),
            ),
        }
    }

    /// The address scripts should dial for the selected listener.
    #[must_use]
    pub(crate) fn listener_addr(self, handle: &Handle) -> Option<SocketAddr> {
        match self.kind {
            TransportKind::Udp | TransportKind::Tcp => Some(handle.local_addr()),
            TransportKind::Tls => handle.tls_addr(),
            TransportKind::Ws => handle.ws_addr(),
            TransportKind::Wss => handle.wss_addr(),
            TransportKind::Quic => None,
        }
    }
}

/// Stable lower-case spelling used by flags and reports.
#[must_use]
pub(crate) fn name(kind: TransportKind) -> &'static str {
    match kind {
        TransportKind::Udp => "udp",
        TransportKind::Tcp => "tcp",
        TransportKind::Tls => "tls",
        TransportKind::Ws => "ws",
        TransportKind::Wss => "wss",
        TransportKind::Quic => "quic",
    }
}

/// Read the optional certificate/key pair, requiring both halves whenever either is present.
fn identity(options: &SignallingOptions, required: bool) -> Result<Option<Identity>, String> {
    let cert = options.identity.tls_cert.as_deref();
    let key = options.identity.tls_key.as_deref();
    match (cert, key) {
        (Some(cert), Some(key)) => {
            let cert_pem = read(cert, "certificate")?;
            let key_pem = read(key, "private key")?;
            Identity::from_pem(&cert_pem, &key_pem)
                .map(Some)
                .map_err(|error| format!("--tls-cert/--tls-key: {error}"))
        }
        (Some(_), None) => Err("--tls-cert requires --tls-key".to_owned()),
        (None, Some(_)) => Err("--tls-key requires --tls-cert".to_owned()),
        (None, None) if required => Err(
            "--transport tls and --transport wss on answer require --tls-cert and --tls-key"
                .to_owned(),
        ),
        (None, None) => Ok(None),
    }
}

fn read(path: &str, what: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|error| format!("reading {what} {path}: {error}"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::output::Format;

    #[test]
    fn a_secure_uri_cannot_select_cleartext() {
        let options = SignallingOptions {
            transport: crate::cli::TransportOptions {
                transport: Some(TransportChoice::Tcp),
                ..crate::cli::TransportOptions::default()
            },
            ..SignallingOptions::default()
        };
        let error = Selection::from_options(&options, true).expect_err("cleartext refused");
        assert!(error.contains("no downgrade"), "{error}");
    }

    #[test]
    fn the_legacy_tcp_alias_keeps_working() {
        let options = SignallingOptions {
            transport: crate::cli::TransportOptions {
                tcp: true,
                ..crate::cli::TransportOptions::default()
            },
            ..SignallingOptions::default()
        };
        let selected = Selection::from_options(&options, false).expect("selected");
        assert_eq!(selected.kind(), TransportKind::Tcp);
        assert!(!selected.report);
    }

    #[test]
    fn explicit_results_name_requested_and_negotiated_in_both_formats() {
        let options = SignallingOptions {
            transport: crate::cli::TransportOptions {
                transport: Some(TransportChoice::Wss),
                ..crate::cli::TransportOptions::default()
            },
            ..SignallingOptions::default()
        };
        let selected = Selection::from_options(&options, false).expect("selected");
        let report = selected.report(Report::new().text("status", "answered"), TransportKind::Wss);
        for rendered in [report.render(Format::Json), report.render(Format::Text)] {
            assert!(rendered.contains("requested_transport"), "{rendered}");
            assert!(rendered.contains("negotiated_transport"), "{rendered}");
            assert!(rendered.contains("wss"), "{rendered}");
        }
    }

    #[test]
    fn the_default_does_not_change_an_existing_result_record() {
        let selected =
            Selection::from_options(&SignallingOptions::default(), false).expect("selected");
        let before = Report::new().text("status", "answered");
        let expected = before.render(Format::Json);
        assert_eq!(
            selected
                .report(before, TransportKind::Udp)
                .render(Format::Json),
            expected
        );
    }
}
