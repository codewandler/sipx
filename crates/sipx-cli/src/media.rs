//! Diagnostic-phone media policy.
//!
//! This module validates closed command values before a socket is opened and maps them directly
//! to `sipx-call`'s public policy. It does not inspect or construct SDP.

use std::net::SocketAddr;

use sipx_call::{CodecPreference, Codecs, IcePolicy, Keying, MediaPolicy, NegotiatedKeying};
use sipx_media::{Codec, IcePath};
use sipx_transport::TransportKind;

use crate::Args;
use crate::output::Report;

/// A validated media selection and the result vocabulary associated with it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Selection {
    policy: MediaPolicy,
    security: Security,
    report: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Security {
    Auto,
    Plain,
    Sdes,
    DtlsSrtp,
}

impl Security {
    const fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Plain => "plain",
            Self::Sdes => "sdes",
            Self::DtlsSrtp => "dtls-srtp",
        }
    }
}

impl Selection {
    /// Parse and validate the media flags before transport binding.
    pub(crate) fn from_args(args: &Args<'_>, transport: TransportKind) -> Result<Self, String> {
        let preferences = args
            .values("codec")
            .map(codec)
            .collect::<Result<Vec<_>, _>>()?;
        let codecs = if preferences.is_empty() {
            Codecs::default()
        } else {
            Codecs::ordered(&preferences).map_err(|error| error.to_string())?
        };

        let security = match args.value("media-security").unwrap_or("auto") {
            "auto" => Security::Auto,
            "plain" => Security::Plain,
            "sdes" => Security::Sdes,
            "dtls-srtp" => Security::DtlsSrtp,
            value => {
                return Err(format!(
                    "unsupported --media-security {value:?}; expected auto, plain, sdes or dtls-srtp"
                ));
            }
        };
        if security == Security::Sdes && !transport.is_secure() {
            return Err(format!(
                "--media-security sdes requires protected TLS or WSS signalling; {transport:?} would expose the SDES key"
            ));
        }
        if security == Security::DtlsSrtp && !cfg!(feature = "dtls") {
            return Err(
                "--media-security dtls-srtp requires a build with the `dtls` feature".to_owned(),
            );
        }

        let ice = match args.value("ice").unwrap_or("disabled") {
            "disabled" => {
                if args.value("stun-server").is_some() {
                    return Err("--stun-server requires --ice stun".to_owned());
                }
                IcePolicy::Disabled
            }
            "host" => {
                if args.value("stun-server").is_some() {
                    return Err("--stun-server requires --ice stun".to_owned());
                }
                IcePolicy::Host
            }
            "stun" => IcePolicy::Stun(
                args.value("stun-server")
                    .ok_or_else(|| "--ice stun requires --stun-server host:port".to_owned())?
                    .parse::<SocketAddr>()
                    .map_err(|_| "--stun-server must be host:port".to_owned())?,
            ),
            value => {
                return Err(format!(
                    "unsupported --ice {value:?}; expected disabled, host or stun"
                ));
            }
        };
        if security == Security::DtlsSrtp && ice != IcePolicy::Disabled {
            return Err(
                "DTLS-SRTP and ICE cannot yet share the initial media port; no fallback is permitted"
                    .to_owned(),
            );
        }

        let keying = match security {
            Security::Auto => Keying::Auto,
            Security::Plain => Keying::Plain,
            Security::Sdes => Keying::Sdes,
            Security::DtlsSrtp => Keying::DtlsSrtp,
        };
        Ok(Self {
            policy: MediaPolicy::default()
                .with_codecs(codecs)
                .with_ice(ice)
                .with_keying(keying),
            security,
            report: args.value("codec").is_some()
                || args.value("media-security").is_some()
                || args.value("ice").is_some()
                || args.value("stun-server").is_some(),
        })
    }

    /// The exact public call policy to pass to dial or answer.
    #[must_use]
    pub(crate) const fn policy(self) -> MediaPolicy {
        self.policy
    }

    /// Add requested values to a listener announcement or terminal result.
    #[must_use]
    pub(crate) fn requested_report(self, report: Report) -> Report {
        if !self.report {
            return report;
        }
        report
            .text("requested_codecs", codec_names(self.policy.codecs))
            .text("requested_media_security", self.security.name())
            .text("requested_ice", ice_name(self.policy.ice))
    }

    /// Add values read from the established call, not inferred from the offer.
    #[must_use]
    pub(crate) fn negotiated_report(self, report: Report, call: &sipx_call::Call) -> Report {
        if !self.report {
            return report;
        }
        let security = match call.negotiated_keying() {
            NegotiatedKeying::Plain => "plain",
            NegotiatedKeying::Sdes => "sdes",
            NegotiatedKeying::DtlsSrtp => "dtls-srtp",
        };
        report
            .text("negotiated_codec", codec_name(call.media().codec()))
            .text("negotiated_media_security", security)
            .text("negotiated_ice", path_name(call.media().ice_path()))
    }
}

fn codec(value: &str) -> Result<CodecPreference, String> {
    match value {
        "pcmu" => Ok(CodecPreference::Pcmu),
        "pcma" => Ok(CodecPreference::Pcma),
        "opus" => Ok(CodecPreference::Opus),
        _ => Err(format!(
            "unsupported --codec {value:?}; expected pcmu, pcma or opus"
        )),
    }
}

fn codec_names(codecs: Codecs) -> String {
    codecs
        .preferences()
        .map(CodecPreference::name)
        .collect::<Vec<_>>()
        .join(",")
}

const fn codec_name(codec: Codec) -> &'static str {
    match codec {
        Codec::Pcmu => "pcmu",
        Codec::Pcma => "pcma",
        #[cfg(feature = "opus")]
        Codec::Opus => "opus",
    }
}

const fn ice_name(ice: IcePolicy) -> &'static str {
    match ice {
        IcePolicy::Disabled => "disabled",
        IcePolicy::Host => "host",
        IcePolicy::Stun(_) => "stun",
    }
}

const fn path_name(path: IcePath) -> &'static str {
    match path {
        IcePath::Disabled => "disabled",
        IcePath::Checking => "checking",
        IcePath::Host => "host",
        IcePath::ServerReflexive => "server-reflexive",
        IcePath::PeerReflexive => "peer-reflexive",
        IcePath::Relayed => "relayed",
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn raw(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    fn selection(raw: &[String], transport: TransportKind) -> Result<Selection, String> {
        Selection::from_args(&Args::new(raw)?, transport)
    }

    #[test]
    fn defaults_map_to_the_call_policy_without_enabling_result_fields() {
        let raw = raw(&["dial", "sip:bob@192.0.2.1"]);
        let selected = selection(&raw, TransportKind::Udp).unwrap();
        assert_eq!(selected.policy(), MediaPolicy::default());
        assert!(!selected.report);
    }

    #[test]
    fn repeated_codecs_reach_the_call_policy_in_order() {
        let raw = raw(&[
            "dial",
            "sip:bob@192.0.2.1",
            "--codec",
            "pcma",
            "--codec",
            "pcmu",
        ]);
        let selected = selection(&raw, TransportKind::Udp).unwrap();
        assert_eq!(
            selected.policy().codecs.preferences().collect::<Vec<_>>(),
            [CodecPreference::Pcma, CodecPreference::Pcmu]
        );
    }

    #[test]
    fn explicit_sdes_on_clear_signalling_is_refused_before_a_call() {
        let raw = raw(&["dial", "sip:bob@192.0.2.1", "--media-security", "sdes"]);
        let error = selection(&raw, TransportKind::Udp).unwrap_err();
        assert!(error.contains("requires protected"), "{error}");
    }

    #[test]
    fn stun_requires_a_server_and_other_policies_refuse_one() {
        let missing = raw(&["dial", "sip:bob@192.0.2.1", "--ice", "stun"]);
        assert!(selection(&missing, TransportKind::Udp).is_err());
        let stray = raw(&[
            "dial",
            "sip:bob@192.0.2.1",
            "--ice",
            "host",
            "--stun-server",
            "127.0.0.1:3478",
        ]);
        assert!(selection(&stray, TransportKind::Udp).is_err());
    }

    #[cfg(not(feature = "opus"))]
    #[test]
    fn opus_is_refused_by_policy_when_the_binary_cannot_run_it() {
        let raw = raw(&["dial", "sip:bob@192.0.2.1", "--codec", "opus"]);
        let error = selection(&raw, TransportKind::Udp).unwrap_err();
        assert!(error.contains("`opus` feature"), "{error}");
    }
}
