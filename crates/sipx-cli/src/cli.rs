//! Declarative command-line contract for the `sipx` binary.
//!
//! This module is the only owner of command names, option names, aliases, value shapes and help.
//! Command modules receive these typed values and apply only cross-field or protocol policy.

use std::net::{IpAddr, SocketAddr};

use clap::{ArgAction, Args as ClapArgs, CommandFactory, Parser, Subcommand, ValueEnum};

use crate::output::Format;

/// Signalling transports exposed by the diagnostic phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum TransportChoice {
    Udp,
    Tcp,
    Tls,
    Ws,
    Wss,
}

/// Media policy profiles exposed by interactive call commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum MediaProfileChoice {
    Standard,
    BrowserAudio,
}

/// Ordered codec choices accepted by the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CodecChoice {
    Pcmu,
    Pcma,
    G722,
    L16,
    Opus,
}

/// Media keying choices accepted by the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum MediaSecurityChoice {
    Auto,
    Plain,
    Sdes,
    DtlsSrtp,
}

/// ICE gathering choices accepted by the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum IceChoice {
    Disabled,
    Host,
    Stun,
}

/// Workload shared by the bounded load caller and responder.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub(crate) enum WorkloadMode {
    /// Bodyless INVITE/ACK/BYE measurement with no media ownership.
    #[default]
    Signalling,
    /// Deterministic PCMU offer/answer and RTP measurement.
    GeneratedMedia,
}

impl WorkloadMode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Signalling => "signalling",
            Self::GeneratedMedia => "generated-media",
        }
    }
}

impl std::fmt::Display for WorkloadMode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The complete command line.
#[derive(Debug, Parser)]
#[command(
    name = "sipx",
    version,
    about = "A command line SIP softphone",
    long_about = "A scriptable command line SIP softphone. Results go to stdout; logs and diagnostics go to stderr.",
    help_template = "{before-help}{about-with-newline}\nUSAGE:\n    {usage}\n\n{all-args}{after-help}"
)]
pub(crate) struct Cli {
    /// Report command results as JSON on stdout.
    #[arg(long, global = true)]
    pub(crate) json: bool,

    /// Show call/load progress; repeat for protocol detail.
    #[arg(short = 'v', action = ArgAction::Count, global = true)]
    pub(crate) verbose: u8,

    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

/// Shipped commands.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Register with a registrar.
    Register(RegisterOptions),
    /// Place a call.
    Dial(DialOptions),
    /// Wait for and answer a call.
    Answer(AnswerOptions),
    /// List stable audio device identifiers.
    #[command(
        long_about = "List stable audio device identifiers.\n\nThe command opens no stream. Device support requires the `device-audio` build feature."
    )]
    Devices(DevicesOptions),
    /// Place a finite, reproducible call load.
    Load(LoadOptions),
    /// Answer a finite, bounded signalling load.
    #[command(name = "load-responder")]
    LoadResponder(LoadResponderOptions),
    /// List what can be called.
    Peers(PeersOptions),
    /// Drive one call through correlated NDJSON commands.
    ///
    /// Each input line is a flat object with a unique string `id` and string `command`.
    /// `wait_for` also requires a finite `timeout_ms`. For example, against an answering peer:
    ///
    /// printf '%s\n' '{"id":"dial-1","command":"dial","uri":"sip:echo@127.0.0.1:5060","timeout_ms":5000}' '{"id":"wait-1","command":"wait_for","event":"call.answered","timeout_ms":5000}' '{"id":"hangup-1","command":"hangup"}' '{"id":"shutdown-1","command":"shutdown"}' | sipx scenario --local 127.0.0.1:0
    Scenario(ScenarioOptions),
    /// Show the binary version.
    Version(VersionOptions),
}

/// Transport options shared by every signalling command.
#[derive(Debug, Clone, Default, ClapArgs)]
pub(crate) struct TransportOptions {
    /// Signalling transport.
    #[arg(long, conflicts_with = "tcp")]
    pub(crate) transport: Option<TransportChoice>,

    /// Legacy alias for `--transport tcp`.
    #[arg(long, conflicts_with = "transport")]
    pub(crate) tcp: bool,
}

/// Certificate identity shared by signalling clients and listeners.
#[derive(Debug, Clone, Default, ClapArgs)]
pub(crate) struct TlsIdentityOptions {
    /// Certificate chain for TLS or WSS.
    #[arg(
        long,
        value_name = "FILE",
        value_parser = parse_non_empty,
        requires = "tls_key"
    )]
    pub(crate) tls_cert: Option<String>,

    /// Private key paired with `--tls-cert`.
    #[arg(
        long,
        value_name = "FILE",
        value_parser = parse_non_empty,
        requires = "tls_cert"
    )]
    pub(crate) tls_key: Option<String>,
}

/// TLS peer-verification options available only to signalling clients.
#[derive(Debug, Clone, Default, ClapArgs)]
pub(crate) struct PeerTlsOptions {
    /// Certificate identity to verify for TLS or WSS.
    #[arg(long, value_parser = parse_non_empty)]
    pub(crate) tls_server_name: Option<String>,

    /// Add PEM trust roots to the platform store.
    #[arg(long, value_name = "FILE", value_parser = parse_non_empty)]
    pub(crate) tls_ca: Option<String>,
}

/// Options shared by commands that initiate signalling connections.
#[derive(Debug, Clone, Default, ClapArgs)]
pub(crate) struct SignallingOptions {
    #[command(flatten)]
    pub(crate) transport: TransportOptions,
    #[command(flatten)]
    pub(crate) identity: TlsIdentityOptions,
    #[command(flatten)]
    pub(crate) peer: PeerTlsOptions,
}

/// Signalling options for a listener, which has no outbound peer to verify.
#[derive(Debug, Clone, Default, ClapArgs)]
pub(crate) struct ListenerSignallingOptions {
    #[command(flatten)]
    pub(crate) transport: TransportOptions,
    #[command(flatten)]
    pub(crate) identity: TlsIdentityOptions,
}

impl ListenerSignallingOptions {
    pub(crate) fn complete(&self) -> SignallingOptions {
        SignallingOptions {
            transport: self.transport.clone(),
            identity: self.identity.clone(),
            peer: PeerTlsOptions::default(),
        }
    }
}

/// Media selectors shared by every command that negotiates media.
#[derive(Debug, Clone, Default, ClapArgs)]
pub(crate) struct MediaSelectionOptions {
    /// Ordered codec preference; repeat to preserve preference order.
    #[arg(long, action = ArgAction::Append)]
    pub(crate) codec: Vec<CodecChoice>,

    /// Media security policy.
    #[arg(long)]
    pub(crate) media_security: Option<MediaSecurityChoice>,

    /// ICE policy.
    #[arg(long)]
    pub(crate) ice: Option<IceChoice>,

    /// STUN server for `--ice stun`, as host:port.
    #[arg(long)]
    pub(crate) stun_server: Option<SocketAddr>,
}

/// Media options for interactive call commands.
#[derive(Debug, Clone, Default, ClapArgs)]
pub(crate) struct MediaOptions {
    /// Media profile.
    #[arg(long)]
    pub(crate) profile: Option<MediaProfileChoice>,
    #[command(flatten)]
    pub(crate) selection: MediaSelectionOptions,
}

/// Scenario media options; the browser profile is not part of the scenario protocol.
#[derive(Debug, Clone, Default, ClapArgs)]
pub(crate) struct ScenarioMediaOptions {
    #[command(flatten)]
    pub(crate) selection: MediaSelectionOptions,
}

impl ScenarioMediaOptions {
    pub(crate) fn complete(&self) -> MediaOptions {
        MediaOptions {
            profile: None,
            selection: self.selection.clone(),
        }
    }
}

/// Options shared by commands that select local audio endpoints.
#[derive(Debug, Clone, Default, ClapArgs)]
pub(crate) struct AudioOptions {
    /// Play mono 16-bit WAV audio.
    #[arg(
        long,
        value_name = "FILE",
        value_parser = parse_non_empty,
        conflicts_with = "audio_input"
    )]
    pub(crate) play: Option<String>,

    /// Record far-end audio to a WAV file.
    #[arg(
        long,
        value_name = "FILE",
        value_parser = parse_non_empty,
        conflicts_with = "audio_output"
    )]
    pub(crate) record: Option<String>,

    /// Local source: `wav:<path>`, `device:<id>` or null.
    #[arg(long, value_parser = parse_non_empty)]
    pub(crate) audio_input: Option<String>,

    /// Local sink: `wav:<path>`, `device:<id>` or null.
    #[arg(long, value_parser = parse_non_empty)]
    pub(crate) audio_output: Option<String>,
}

/// Signalling capture and counter export paths.
#[derive(Debug, Clone, Default, ClapArgs)]
pub(crate) struct CaptureOptions {
    /// Record redacted signalling to this pcapng file.
    #[arg(long, value_name = "FILE", value_parser = parse_non_empty)]
    pub(crate) capture: Option<String>,

    /// Write signalling counters as JSON.
    #[arg(long, value_name = "FILE", value_parser = parse_non_empty)]
    pub(crate) counters: Option<String>,
}

/// Repeatable application-owned SIP fields.
#[derive(Debug, Clone, Default, ClapArgs)]
pub(crate) struct HeaderOptions {
    /// Add an application-owned SIP field as `Name: value`.
    #[arg(long, action = ArgAction::Append, value_parser = parse_non_empty)]
    pub(crate) header: Vec<String>,
}

/// `sipx register`.
#[derive(Debug, ClapArgs)]
pub(crate) struct RegisterOptions {
    /// Address of record, for example sip:alice@example.com.
    pub(crate) aor: String,
    #[arg(
        long,
        env = "SIPX_PASSWORD",
        hide_env_values = true,
        value_parser = parse_non_empty
    )]
    pub(crate) password: Option<String>,
    /// Destination when it cannot be derived from the AOR, as host:port.
    #[arg(long, value_parser = parse_non_empty)]
    pub(crate) target: Option<String>,
    /// Requested lease in seconds.
    #[arg(long, default_value_t = 3600, value_name = "S", value_parser = parse_seconds)]
    pub(crate) expires: u64,
    /// Local address to bind.
    #[arg(long, default_value = "0.0.0.0:0")]
    pub(crate) local: SocketAddr,
    #[command(flatten)]
    pub(crate) signalling: SignallingOptions,
    #[command(flatten)]
    pub(crate) headers: HeaderOptions,
    #[arg(long)]
    pub(crate) keep_alive: bool,
    #[arg(long)]
    pub(crate) outbound: bool,
    #[arg(long, value_parser = parse_non_empty)]
    pub(crate) instance: Option<String>,
    #[arg(long, value_parser = parse_non_empty)]
    pub(crate) push_provider: Option<String>,
    #[arg(long, value_parser = parse_non_empty)]
    pub(crate) push_prid: Option<String>,
    #[arg(long, value_parser = parse_non_empty)]
    pub(crate) push_param: Option<String>,
    #[arg(long)]
    pub(crate) wake: bool,
    #[command(flatten)]
    pub(crate) capture: CaptureOptions,
}

/// `sipx dial`.
#[derive(Debug, ClapArgs)]
pub(crate) struct DialOptions {
    /// SIP URI to call.
    pub(crate) uri: String,
    #[command(flatten)]
    pub(crate) audio: AudioOptions,
    /// DTMF digits to send once connected.
    #[arg(long, value_parser = parse_non_empty)]
    pub(crate) dtmf: Option<String>,
    /// Connected-call duration in seconds; a supported process stop hangs up early.
    #[arg(long, default_value_t = 30, value_name = "S", value_parser = parse_seconds)]
    pub(crate) duration: u64,
    /// Answer timeout in seconds; zero delegates to the transaction layer.
    #[arg(long, default_value_t = 20, value_name = "S", value_parser = parse_seconds)]
    pub(crate) timeout: u64,
    /// Additional invitation-cancellation allowance in seconds.
    #[arg(long, default_value_t = 2, value_name = "S", value_parser = parse_seconds)]
    pub(crate) cancel_timeout: u64,
    #[arg(long, value_parser = parse_non_empty)]
    pub(crate) from: Option<String>,
    #[arg(
        long,
        env = "SIPX_PASSWORD",
        hide_env_values = true,
        value_parser = parse_non_empty
    )]
    pub(crate) password: Option<String>,
    #[arg(long, default_value = "0.0.0.0:0")]
    pub(crate) local: SocketAddr,
    #[arg(long)]
    pub(crate) advertise: Option<IpAddr>,
    #[command(flatten)]
    pub(crate) signalling: SignallingOptions,
    #[command(flatten)]
    pub(crate) media: MediaOptions,
    #[arg(long)]
    pub(crate) early_media: bool,
    #[command(flatten)]
    pub(crate) headers: HeaderOptions,
    #[arg(long)]
    pub(crate) stats: bool,
    #[command(flatten)]
    pub(crate) capture: CaptureOptions,
}

/// `sipx answer`.
#[derive(Debug, ClapArgs)]
pub(crate) struct AnswerOptions {
    #[command(flatten)]
    pub(crate) audio: AudioOptions,
    /// Maximum call duration; remote BYE or a supported process stop ends it early.
    #[arg(long, default_value_t = 30, value_name = "S", value_parser = parse_seconds)]
    pub(crate) duration: u64,
    #[arg(long, default_value_t = 60, value_name = "S", value_parser = parse_seconds)]
    pub(crate) wait: u64,
    #[arg(long, default_value = "0.0.0.0:5060")]
    pub(crate) local: SocketAddr,
    #[arg(long)]
    pub(crate) advertise: Option<IpAddr>,
    #[command(flatten)]
    pub(crate) signalling: ListenerSignallingOptions,
    #[command(flatten)]
    pub(crate) media: MediaOptions,
    #[command(flatten)]
    pub(crate) headers: HeaderOptions,
    #[arg(long, conflicts_with = "busy")]
    pub(crate) reject: bool,
    #[arg(long, conflicts_with = "reject")]
    pub(crate) busy: bool,
    /// Exit after one call; retained for script clarity.
    #[arg(long)]
    pub(crate) once: bool,
    #[command(flatten)]
    pub(crate) capture: CaptureOptions,
}

/// `sipx devices`.
#[derive(Debug, ClapArgs)]
pub(crate) struct DevicesOptions {}

/// `sipx load`.
#[derive(Debug, ClapArgs)]
pub(crate) struct LoadOptions {
    /// SIP URI called by every admitted call.
    pub(crate) uri: String,
    /// Positive finite calls per second.
    #[arg(long)]
    pub(crate) rate: f64,
    /// Positive maximum active calls.
    #[arg(long)]
    pub(crate) concurrency: usize,
    /// Stop after admitting this many calls.
    #[arg(long)]
    pub(crate) calls: Option<usize>,
    /// Stop admission after this many seconds.
    #[arg(long, value_name = "S", value_parser = parse_seconds)]
    pub(crate) duration: Option<u64>,
    /// End each answered call after this many seconds.
    #[arg(long, default_value_t = 0, value_name = "S", value_parser = parse_seconds)]
    pub(crate) call_duration: u64,
    /// Bound each call setup in seconds.
    #[arg(long, default_value_t = 20, value_name = "S", value_parser = parse_seconds)]
    pub(crate) timeout: u64,
    /// Select bodyless signalling or deterministic generated media.
    #[arg(long, default_value_t)]
    pub(crate) mode: WorkloadMode,
    #[arg(long, default_value_t = 0)]
    pub(crate) seed: u64,
    #[arg(long, value_parser = parse_non_empty)]
    pub(crate) from: Option<String>,
    #[arg(
        long,
        env = "SIPX_PASSWORD",
        hide_env_values = true,
        value_parser = parse_non_empty
    )]
    pub(crate) password: Option<String>,
    #[arg(long, default_value = "0.0.0.0:0")]
    pub(crate) local: SocketAddr,
    #[command(flatten)]
    pub(crate) signalling: SignallingOptions,
}

/// `sipx load-responder`.
#[derive(Debug, ClapArgs)]
pub(crate) struct LoadResponderOptions {
    #[arg(long)]
    pub(crate) max_active: usize,
    #[arg(long)]
    pub(crate) calls: Option<usize>,
    #[arg(long, value_name = "S", value_parser = parse_seconds)]
    pub(crate) duration: Option<u64>,
    #[arg(long, value_name = "S", value_parser = parse_seconds)]
    pub(crate) cleanup: u64,
    #[arg(long, default_value_t = 0)]
    pub(crate) seed: u64,
    #[arg(long, default_value_t = 0)]
    pub(crate) provisional_percent: u8,
    #[arg(long, default_value_t = 100)]
    pub(crate) answer_percent: u8,
    #[arg(long, default_value_t = 486)]
    pub(crate) reject_status: u16,
    #[arg(long, default_value_t = 40, value_name = "S", value_parser = parse_seconds)]
    pub(crate) dialog_duration: u64,
    /// Select bodyless signalling or deterministic generated media.
    #[arg(long, default_value_t)]
    pub(crate) mode: WorkloadMode,
    #[arg(long, default_value = "127.0.0.1:0")]
    pub(crate) local: SocketAddr,
    #[arg(long, default_value = "udp", value_parser = ["udp"])]
    pub(crate) transport: String,
}

/// `sipx peers`.
#[derive(Debug, ClapArgs)]
pub(crate) struct PeersOptions {
    #[arg(
        long,
        env = "SIPX_PEERS",
        value_name = "FILE",
        value_parser = parse_non_empty
    )]
    pub(crate) book: Option<String>,
    #[arg(long, value_parser = parse_non_empty)]
    pub(crate) registrar: Option<String>,
    #[arg(
        long,
        env = "SIPX_PASSWORD",
        hide_env_values = true,
        value_parser = parse_non_empty
    )]
    pub(crate) password: Option<String>,
    #[arg(long, value_parser = parse_non_empty)]
    pub(crate) target: Option<String>,
    #[arg(long, default_value_t = 3600, value_name = "S", value_parser = parse_seconds)]
    pub(crate) expires: u64,
    #[arg(long, default_value_t = 0, value_name = "S", value_parser = parse_seconds)]
    pub(crate) watch: u64,
    #[arg(long, default_value = "0.0.0.0:0")]
    pub(crate) local: SocketAddr,
    #[command(flatten)]
    pub(crate) signalling: SignallingOptions,
}

/// `sipx scenario`.
#[derive(Debug, ClapArgs)]
pub(crate) struct ScenarioOptions {
    #[arg(long, default_value = "0.0.0.0:0")]
    pub(crate) local: SocketAddr,
    #[command(flatten)]
    pub(crate) signalling: SignallingOptions,
    #[command(flatten)]
    pub(crate) media: ScenarioMediaOptions,
    #[command(flatten)]
    pub(crate) headers: HeaderOptions,
    #[arg(long, default_value_t = 20, value_name = "S", value_parser = parse_seconds)]
    pub(crate) timeout: u64,
}

/// `sipx version`.
#[derive(Debug, ClapArgs)]
pub(crate) struct VersionOptions {}

impl Cli {
    /// Parse the process command line without exposing raw argv to the application.
    pub(crate) fn parse_process() -> Result<Self, clap::Error> {
        Self::try_parse()
    }

    /// Render root help from the same model that parses it.
    pub(crate) fn root_help() -> String {
        Self::command().render_long_help().to_string()
    }

    /// Recover the requested result format after a parse error using this same parser model.
    pub(crate) fn requested_format() -> Format {
        requested_format_from(
            Self::command()
                .ignore_errors(true)
                .allow_external_subcommands(true)
                .try_get_matches(),
        )
    }
}

fn requested_format_from(matches: Result<clap::ArgMatches, clap::Error>) -> Format {
    let Ok(matches) = matches else {
        return Format::Text;
    };
    if matches.get_flag("json") {
        return Format::Json;
    }

    let Some((_, subcommand)) = matches.subcommand() else {
        return Format::Text;
    };
    if subcommand.get_flag("json") {
        return Format::Json;
    }

    // An unknown subcommand is deliberately captured so a parse error can still use the requested
    // output format. Clap preserves that command's tail as external values; feed the tail back into
    // this same command model so `--json` is still recognized by its one declarative definition.
    // Every recursion has consumed one external command name, so malformed input remains bounded.
    let Ok(Some(external)) = subcommand.try_get_raw("") else {
        return Format::Text;
    };
    requested_format_from(
        Cli::command()
            .no_binary_name(true)
            .ignore_errors(true)
            .allow_external_subcommands(true)
            .try_get_matches_from(external),
    )
}

fn parse_non_empty(raw: &str) -> Result<String, String> {
    if raw.is_empty() {
        Err("value cannot be empty".to_owned())
    } else {
        Ok(raw.to_owned())
    }
}

fn parse_seconds(raw: &str) -> Result<u64, String> {
    let value = raw
        .parse::<u64>()
        .map_err(|_| format!("expected a whole number of seconds, not {raw:?}"))?;
    if value > u64::from(u32::MAX) {
        return Err(format!(
            "seconds must be in the range 0 through {}, not {raw:?}",
            u32::MAX
        ));
    }
    Ok(value)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn repeated_codecs_keep_command_line_order() {
        let parsed = Cli::try_parse_from([
            "sipx",
            "dial",
            "sip:bob@example.net",
            "--codec",
            "pcma",
            "--codec",
            "pcmu",
        ])
        .expect("valid command");
        let Some(Command::Dial(options)) = parsed.command else {
            panic!("dial command expected");
        };
        assert_eq!(
            options.media.selection.codec,
            [CodecChoice::Pcma, CodecChoice::Pcmu]
        );
    }

    #[test]
    fn clustered_verbosity_is_counted_by_the_parser() {
        let parsed = Cli::try_parse_from(["sipx", "-vv", "devices"]).expect("valid command");
        assert_eq!(parsed.verbose, 2);
    }

    #[test]
    fn conflicting_alias_and_transport_are_refused() {
        assert!(
            Cli::try_parse_from([
                "sipx",
                "dial",
                "sip:bob@example.net",
                "--tcp",
                "--transport",
                "udp",
            ])
            .is_err()
        );
    }

    #[test]
    fn equals_form_keeps_a_value_that_begins_with_a_dash() {
        let parsed = Cli::try_parse_from([
            "sipx",
            "register",
            "sip:alice@example.net",
            "--password=-secret",
        ])
        .expect("valid command");
        let Some(Command::Register(options)) = parsed.command else {
            panic!("register command expected");
        };
        assert_eq!(options.password.as_deref(), Some("-secret"));
    }

    #[test]
    fn a_missing_or_empty_value_is_refused_by_the_parser() {
        for arguments in [
            vec!["sipx", "register", "sip:alice@example.net", "--password"],
            vec!["sipx", "register", "sip:alice@example.net", "--password="],
        ] {
            let error = Cli::try_parse_from(arguments).expect_err("missing value refused");
            assert!(error.to_string().contains("--password"), "{error}");
        }
    }

    #[test]
    fn help_wins_over_other_malformed_input() {
        let error = Cli::try_parse_from(["sipx", "dial", "--help", "--play"])
            .expect_err("help is represented as a successful-display outcome");
        assert_eq!(error.kind(), clap::error::ErrorKind::DisplayHelp);
        let help = error.to_string();
        assert!(help.contains("Usage: sipx dial"), "{help}");
        assert!(!help.contains("a value is required"), "{help}");
    }

    #[test]
    fn shared_transport_option_reaches_multiple_typed_commands() {
        let dial =
            Cli::try_parse_from(["sipx", "dial", "sip:bob@example.net", "--transport", "tcp"])
                .expect("valid dial command");
        let register = Cli::try_parse_from([
            "sipx",
            "register",
            "sip:alice@example.net",
            "--transport",
            "tls",
        ])
        .expect("valid register command");

        let Some(Command::Dial(dial)) = dial.command else {
            panic!("dial command expected");
        };
        let Some(Command::Register(register)) = register.command else {
            panic!("register command expected");
        };
        assert_eq!(
            dial.signalling.transport.transport,
            Some(TransportChoice::Tcp)
        );
        assert_eq!(
            register.signalling.transport.transport,
            Some(TransportChoice::Tls)
        );
    }

    #[test]
    fn parse_error_format_comes_from_the_same_global_option_model() {
        let matches = Cli::command()
            .ignore_errors(true)
            .allow_external_subcommands(true)
            .try_get_matches_from(["sipx", "frobnicate", "--json"]);
        assert_eq!(requested_format_from(matches), Format::Json);
    }
}
