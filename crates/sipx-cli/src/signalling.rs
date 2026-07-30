//! Signalling transport policy shared by every command that opens an endpoint.
//!
//! The command layer decides what the user asked for before any socket is opened, then hands the
//! existing transport types the trust anchors, identity and verification name. Keeping that policy
//! here prevents `dial`, `register` and `answer` from acquiring three subtly different downgrade
//! rules.

use std::net::SocketAddr;

use sipx_transport::tls::{ClientTls, Identity, ServerTls, TrustAnchors};
use sipx_transport::{Config, Handle, Target, TransportKind};

use crate::Args;
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
    pub(crate) fn from_args(args: &Args<'_>, secure_uri: bool) -> Result<Self, String> {
        let requested = args.value("transport");
        if requested.is_some() && args.flag("tcp") {
            return Err(
                "--transport and the legacy --tcp alias cannot be used together".to_owned(),
            );
        }

        let kind = match requested {
            Some(token) => parse(token)?,
            None if args.flag("tcp") => TransportKind::Tcp,
            None => TransportKind::Udp,
        };
        if secure_uri && !kind.is_secure() {
            return Err(format!(
                "a sips: URI requires --transport tls or --transport wss; {} is cleartext and no downgrade is permitted",
                name(kind)
            ));
        }

        let has_tls_option = ["tls-server-name", "tls-ca", "tls-cert", "tls-key"]
            .iter()
            .any(|option| args.value(option).is_some());
        if has_tls_option && !kind.is_secure() {
            return Err(format!(
                "TLS identity and trust options require --transport tls or --transport wss, not {}",
                name(kind)
            ));
        }
        identity(args, false)?;

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
        args: &Args<'_>,
        addr: SocketAddr,
        default_server_name: &str,
    ) -> Result<Target, String> {
        let target = Target::new(addr, self.kind);
        if self.kind.is_secure() {
            let verify_as = args.value("tls-server-name").unwrap_or(default_server_name);
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
        args: &Args<'_>,
        config: &mut Config,
    ) -> Result<(), String> {
        if !self.kind.is_secure() {
            return Ok(());
        }

        let mut anchors = TrustAnchors::system();
        if let Some(path) = args.value("tls-ca") {
            let pem = read(path, "trust roots")?;
            anchors
                .add_pem(&pem)
                .map_err(|error| format!("--tls-ca {path}: {error}"))?;
        }
        let client_identity = identity(args, false)?;
        config.tls_client = Some(
            ClientTls::with_identity(&anchors, client_identity)
                .map_err(|error| format!("TLS client configuration: {error}"))?,
        );
        Ok(())
    }

    /// Configure the one listener `answer --transport` promises.
    pub(crate) fn configure_listener(
        self,
        args: &Args<'_>,
        config: &mut Config,
    ) -> Result<(), String> {
        if args.value("tls-server-name").is_some() || args.value("tls-ca").is_some() {
            return Err(
                "--tls-server-name and --tls-ca configure an outbound TLS peer and are not valid for answer"
                    .to_owned(),
            );
        }
        if self.report {
            config.tcp = self.kind == TransportKind::Tcp;
        }
        match self.kind {
            TransportKind::Udp | TransportKind::Tcp => Ok(()),
            TransportKind::Tls => {
                let server = ServerTls::new(
                    identity(args, true)?.ok_or_else(|| "TLS identity is required".to_owned())?,
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
                    identity(args, true)?.ok_or_else(|| "TLS identity is required".to_owned())?,
                )
                .map_err(|error| format!("TLS server configuration: {error}"))?;
                config.wss_server = Some((server, config.bind.port()));
                Ok(())
            }
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
    }
}

fn parse(token: &str) -> Result<TransportKind, String> {
    match token {
        "udp" => Ok(TransportKind::Udp),
        "tcp" => Ok(TransportKind::Tcp),
        "tls" => Ok(TransportKind::Tls),
        "ws" => Ok(TransportKind::Ws),
        "wss" => Ok(TransportKind::Wss),
        _ => Err(format!(
            "--transport must be one of udp, tcp, tls, ws or wss; got {token}"
        )),
    }
}

/// Read the optional certificate/key pair, requiring both halves whenever either is present.
fn identity(args: &Args<'_>, required: bool) -> Result<Option<Identity>, String> {
    let cert = args.value("tls-cert");
    let key = args.value("tls-key");
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

    fn arguments(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn a_secure_uri_cannot_select_cleartext() {
        let raw = arguments(&["dial", "sip:a@b", "--transport", "tcp"]);
        let args = Args::new(&raw).expect("arguments");
        let error = Selection::from_args(&args, true).expect_err("cleartext refused");
        assert!(error.contains("no downgrade"), "{error}");
    }

    #[test]
    fn the_legacy_tcp_alias_keeps_working_but_cannot_conflict() {
        let raw = arguments(&["dial", "sip:a@b", "--tcp"]);
        let args = Args::new(&raw).expect("arguments");
        let selected = Selection::from_args(&args, false).expect("selected");
        assert_eq!(selected.kind(), TransportKind::Tcp);
        assert!(!selected.report);

        let raw = arguments(&["dial", "sip:a@b", "--tcp", "--transport", "tcp"]);
        let args = Args::new(&raw).expect("arguments");
        assert!(Selection::from_args(&args, false).is_err());
    }

    #[test]
    fn explicit_results_name_requested_and_negotiated_in_both_formats() {
        let raw = arguments(&["dial", "sip:a@b", "--transport", "wss"]);
        let args = Args::new(&raw).expect("arguments");
        let selected = Selection::from_args(&args, false).expect("selected");
        let report = selected.report(Report::new().text("status", "answered"), TransportKind::Wss);
        for rendered in [report.render(Format::Json), report.render(Format::Text)] {
            assert!(rendered.contains("requested_transport"), "{rendered}");
            assert!(rendered.contains("negotiated_transport"), "{rendered}");
            assert!(rendered.contains("wss"), "{rendered}");
        }
    }

    #[test]
    fn the_default_does_not_change_an_existing_result_record() {
        let raw = arguments(&["dial", "sip:a@b"]);
        let args = Args::new(&raw).expect("arguments");
        let selected = Selection::from_args(&args, false).expect("selected");
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
