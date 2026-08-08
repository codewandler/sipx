//! Diagnostic-phone media policy.
//!
//! This module validates closed command values before a socket is opened and maps them directly
//! to `sipx-call`'s public policy. It does not inspect or construct SDP.

use sipx_call::{
    CodecPreference, Codecs, IcePolicy, Keying, MediaPolicy, MediaProfile, NegotiatedKeying,
};
use sipx_media::browser::ComponentState;
use sipx_media::{Codec, IcePath};
use sipx_transport::TransportKind;

use crate::cli::{CodecChoice, IceChoice, MediaOptions, MediaProfileChoice, MediaSecurityChoice};
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

/// Validate every build-dependent and ICE selector without performing I/O.
pub(crate) fn preflight(options: &MediaOptions) -> Result<(), String> {
    let profile = options.profile.unwrap_or(MediaProfileChoice::Standard);
    if profile == MediaProfileChoice::BrowserAudio {
        if !cfg!(feature = "opus") {
            return Err(
                "--profile browser-audio requires a build with the `opus` feature".to_owned(),
            );
        }
        if !cfg!(feature = "dtls") {
            return Err(
                "--profile browser-audio requires a build with the `dtls` feature".to_owned(),
            );
        }
    }

    let selection = &options.selection;
    if !selection.codec.is_empty() {
        let preferences = selection
            .codec
            .iter()
            .copied()
            .map(codec)
            .collect::<Vec<_>>();
        Codecs::ordered(&preferences).map_err(|error| error.to_string())?;
    }
    if selection.media_security == Some(MediaSecurityChoice::DtlsSrtp) && !cfg!(feature = "dtls") {
        return Err(
            "--media-security dtls-srtp requires a build with the `dtls` feature".to_owned(),
        );
    }
    match selection.ice.unwrap_or(IceChoice::Disabled) {
        IceChoice::Stun if selection.stun_server.is_none() => {
            return Err("--ice stun requires --stun-server host:port".to_owned());
        }
        IceChoice::Disabled | IceChoice::Host if selection.stun_server.is_some() => {
            return Err("--stun-server requires --ice stun".to_owned());
        }
        IceChoice::Disabled | IceChoice::Host | IceChoice::Stun => {}
    }
    if profile == MediaProfileChoice::Standard
        && selection.media_security == Some(MediaSecurityChoice::DtlsSrtp)
        && selection.ice.unwrap_or(IceChoice::Disabled) != IceChoice::Disabled
    {
        return Err(
            "DTLS-SRTP and ICE cannot yet share the initial media port; no fallback is permitted"
                .to_owned(),
        );
    }
    Ok(())
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
    pub(crate) fn from_options(
        options: &MediaOptions,
        transport: TransportKind,
        early_media: bool,
    ) -> Result<Self, String> {
        preflight(options)?;
        let profile = match options.profile.unwrap_or(MediaProfileChoice::Standard) {
            MediaProfileChoice::Standard => MediaProfile::Standard,
            MediaProfileChoice::BrowserAudio => MediaProfile::BrowserAudio,
        };
        if profile == MediaProfile::BrowserAudio {
            return Self::browser_audio(options, transport, early_media);
        }
        let selection = &options.selection;
        let preferences = selection
            .codec
            .iter()
            .copied()
            .map(codec)
            .collect::<Vec<_>>();
        let codecs = if preferences.is_empty() {
            Codecs::default()
        } else {
            Codecs::ordered(&preferences).map_err(|error| error.to_string())?
        };

        let security = match selection
            .media_security
            .unwrap_or(MediaSecurityChoice::Auto)
        {
            MediaSecurityChoice::Auto => Security::Auto,
            MediaSecurityChoice::Plain => Security::Plain,
            MediaSecurityChoice::Sdes => Security::Sdes,
            MediaSecurityChoice::DtlsSrtp => Security::DtlsSrtp,
        };
        if security == Security::Sdes && !transport.is_secure() {
            return Err(format!(
                "--media-security sdes requires protected TLS or WSS signalling; {transport:?} would expose the SDES key"
            ));
        }
        let ice = match selection.ice.unwrap_or(IceChoice::Disabled) {
            IceChoice::Disabled => {
                if selection.stun_server.is_some() {
                    return Err("--stun-server requires --ice stun".to_owned());
                }
                IcePolicy::Disabled
            }
            IceChoice::Host => {
                if selection.stun_server.is_some() {
                    return Err("--stun-server requires --ice stun".to_owned());
                }
                IcePolicy::Host
            }
            IceChoice::Stun => IcePolicy::Stun(
                selection
                    .stun_server
                    .as_ref()
                    .ok_or_else(|| "--ice stun requires --stun-server host:port".to_owned())?
                    .to_owned(),
            ),
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
            report: !selection.codec.is_empty()
                || selection.media_security.is_some()
                || selection.ice.is_some()
                || selection.stun_server.is_some(),
        })
    }

    fn browser_audio(
        options: &MediaOptions,
        transport: TransportKind,
        early_media: bool,
    ) -> Result<Self, String> {
        if transport != TransportKind::Wss {
            return Err("--profile browser-audio requires --transport wss".to_owned());
        }
        if early_media {
            return Err(
                "--profile browser-audio does not support --early-media; wait for the final answer"
                    .to_owned(),
            );
        }
        let selection = &options.selection;
        if !selection.codec.is_empty() || selection.media_security.is_some() {
            return Err(
                "--profile browser-audio fixes codecs and media security; do not combine it with --codec or --media-security"
                    .to_owned(),
            );
        }
        let ice = match selection.ice.unwrap_or(IceChoice::Host) {
            IceChoice::Host => {
                if selection.stun_server.is_some() {
                    return Err("--stun-server requires --ice stun".to_owned());
                }
                IcePolicy::Host
            }
            IceChoice::Stun => IcePolicy::Stun(
                selection
                    .stun_server
                    .as_ref()
                    .ok_or_else(|| "--ice stun requires --stun-server host:port".to_owned())?
                    .to_owned(),
            ),
            IceChoice::Disabled => {
                return Err(
                    "--profile browser-audio requires ICE; disabled is not allowed".to_owned(),
                );
            }
        };
        Ok(Self {
            policy: MediaPolicy::browser_audio().with_ice(ice),
            security: Security::DtlsSrtp,
            report: true,
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
            .text("media_profile", profile_name(self.policy.profile))
            .text("requested_codecs", codec_names(self.policy.codecs))
            .text("requested_media_security", self.security.name())
            .text("requested_ice", ice_name(self.policy.ice))
    }

    /// Add values read from the established call, not inferred from the offer.
    #[must_use]
    pub(crate) fn negotiated_report(
        self,
        report: Report,
        call: &sipx_call::Call,
        browser_role: &str,
    ) -> Report {
        if !self.report {
            return report;
        }
        let security = match call.negotiated_keying() {
            NegotiatedKeying::Plain => "plain",
            NegotiatedKeying::Sdes => "sdes",
            NegotiatedKeying::DtlsSrtp => "dtls-srtp",
        };
        let mut report = report
            .text("negotiated_codec", codec_name(call.media().codec()))
            .number(
                "negotiated_payload_type",
                i64::from(call.negotiated_payload_type()),
            )
            .number(
                "negotiated_clock_rate",
                i64::from(call.negotiated_clock_rate()),
            )
            .text("negotiated_keying", security)
            .text("negotiated_media_security", security)
            .text("negotiated_ice", path_name(call.media().ice_path()));
        if call.media_profile() == MediaProfile::BrowserAudio {
            report = report
                .text("browser_role", browser_role)
                .number("ice_component", 1);
            if let Some(snapshot) = call.browser_component() {
                report = report
                    .text("media_state", component_state(snapshot.state))
                    .number(
                        "ingress_drops_total",
                        i64::try_from(snapshot.counts.total()).unwrap_or(i64::MAX),
                    );
                if let Some(selected) = snapshot.selected {
                    report = report
                        .text("nominated_local", selected.local.to_string())
                        .text("nominated_remote", selected.remote.to_string())
                        .number(
                            "ice_generation",
                            i64::try_from(selected.ice_generation).unwrap_or(i64::MAX),
                        )
                        .text("local_candidate_type", candidate_type(selected.local_kind))
                        .text(
                            "remote_candidate_type",
                            candidate_type(selected.remote_kind),
                        );
                }
            }
        }
        report
    }
}

const fn profile_name(profile: MediaProfile) -> &'static str {
    match profile {
        MediaProfile::Standard => "standard",
        MediaProfile::BrowserAudio => "browser-audio",
    }
}

const fn component_state(state: ComponentState) -> &'static str {
    match state {
        ComponentState::IceChecking => "ice-checking",
        ComponentState::Nominated => "nominated",
        ComponentState::DtlsHandshaking => "dtls-handshaking",
        ComponentState::KeysInstalled => "keys-installed",
        ComponentState::Running => "running",
        ComponentState::Closed => "closed",
    }
}

fn candidate_type(kind: sipx_sdp::ice::CandidateType) -> &'static str {
    kind.as_str()
}

const fn codec(value: CodecChoice) -> CodecPreference {
    match value {
        CodecChoice::Pcmu => CodecPreference::Pcmu,
        CodecChoice::Pcma => CodecPreference::Pcma,
        CodecChoice::G722 => CodecPreference::G722,
        CodecChoice::Opus => CodecPreference::Opus,
        CodecChoice::L16 => CodecPreference::L16,
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
        Codec::G722 => "g722",
        Codec::L16 => "l16",
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
    use clap::Parser as _;

    use crate::cli::{Cli, Command};

    fn raw(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    fn selection(raw: &[String], transport: TransportKind) -> Result<Selection, String> {
        let parsed =
            Cli::try_parse_from(std::iter::once("sipx").chain(raw.iter().map(String::as_str)))
                .map_err(|error| error.to_string())?;
        let (options, early_media) = match parsed.command {
            Some(Command::Dial(options)) => (options.media, options.early_media),
            Some(Command::Answer(options)) => (options.media, false),
            Some(Command::Scenario(options)) => (options.media.complete(), false),
            _ => return Err("media test requires dial, answer or scenario".to_owned()),
        };
        Selection::from_options(&options, transport, early_media)
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
    fn l16_reaches_the_exact_call_policy() {
        let raw = raw(&["dial", "sip:bob@192.0.2.1", "--codec", "l16"]);
        let selected = selection(&raw, TransportKind::Udp).unwrap();
        assert_eq!(selected.policy().codecs, Codecs::L16);
    }

    /// M-44: `--codec g722` is a first-class ordered selection in every build — unlike Opus it
    /// has no feature gate to refuse it behind.
    #[test]
    fn g722_reaches_the_exact_call_policy() {
        let alone = raw(&["dial", "sip:bob@192.0.2.1", "--codec", "g722"]);
        let selected = selection(&alone, TransportKind::Udp).unwrap();
        assert_eq!(
            selected.policy().codecs.preferences().collect::<Vec<_>>(),
            [CodecPreference::G722]
        );

        let ordered = raw(&[
            "dial",
            "sip:bob@192.0.2.1",
            "--codec",
            "g722",
            "--codec",
            "pcmu",
        ]);
        let selected = selection(&ordered, TransportKind::Udp).unwrap();
        assert_eq!(
            selected.policy().codecs.preferences().collect::<Vec<_>>(),
            [CodecPreference::G722, CodecPreference::Pcmu]
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

    #[cfg(not(feature = "opus"))]
    #[test]
    fn browser_audio_is_known_but_refused_when_opus_is_not_built() {
        let raw = raw(&["dial", "sip:bob@192.0.2.1", "--profile", "browser-audio"]);
        let error = selection(&raw, TransportKind::Wss).unwrap_err();
        assert_eq!(
            error,
            "--profile browser-audio requires a build with the `opus` feature"
        );
    }
}
