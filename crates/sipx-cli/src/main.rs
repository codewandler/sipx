//! `sipx` — a command line SIP softphone.
//!
//! Scriptable by design. Every command reports its result as a line of JSON on request, uses a
//! distinct exit code per outcome, and keeps logging off stdout — so a shell can place a call,
//! assert on what happened, and branch on why it did not.
//!
//! # Stability
//!
//! sipx is pre-1.0, so this does not mean frozen; `1.0.0` is what freezes an interface and its
//! predicates are in `docs/roadmap.md`.
//!
//! **This crate's promise is its command-line surface, not its Rust API.** Nothing here is `pub`, it
//! ships no library target, and `cargo doc -p sipx-cli` renders under the binary name — so a reader
//! following a `sipx_cli` link finds nothing. The contract is the commands, flags, environment
//! variables and exit codes documented in `website/docs/reference/cli.md` and asserted in
//! `tests/cli.rs`.
//!
//! **Supported**: `register`, `dial`, `answer`, `load`, `load-responder`, `peers`, optional
//! device-audio selection, their flags, `SIPX_PASSWORD`, the `--book` lookup order, signalling
//! transport selection and the exit codes.
//!
//! Refused rather than silently unsupported, because a flag that is accepted and dropped is worse
//! than one that errors: for example, a cleartext transport for a `sips:` URI.
//!

mod advertise;
mod answer;
mod counters;
mod device;
mod dial;
mod header;
mod load;
mod load_responder;
mod media;
mod output;
mod peers;
mod register;
mod scenario;
mod signalling;

use std::process::ExitCode;

use output::{Exit, Format};

const USAGE: &str = "\
sipx — a command line SIP softphone

USAGE:
    sipx <COMMAND> [OPTIONS]

COMMANDS:
    register    Register with a registrar
    dial        Place a call
    answer      Wait for and answer a call
    devices     List stable audio device identifiers
    load        Place a finite, reproducible call load
    load-responder  Answer a finite, bounded signalling load
    peers       List what can be called
    scenario    Drive a call through correlated NDJSON commands
    help        Show this message
    version     Show the version

GLOBAL OPTIONS:
    --json      Report results as JSON on stdout
    -v          Log a call's progress to stderr: what was dialled, what answered,
                who called, how it ended (INFO)
    -vv         Add the protocol detail behind it: signalling, transactions, media
                (DEBUG, and the most there is — further v's change nothing)
    -h, --help  Show help for a command

    Logging never reaches stdout, which carries results. Repeated v's count by
    letter, so -vv and -v -v are the same request.

EXIT CODES:
    0  success        3  rejected       5  timeout
    1  failed         4  unauthorized   6  busy
    2  usage
";

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let format = if args.iter().any(|a| a == "--json") {
        Format::Json
    } else {
        Format::Text
    };

    // Logging goes to stderr. One stray line on stdout turns valid JSON into a parse error at
    // the far end of a pipe, where the cause is invisible.
    init_logging(verbosity(&args));

    let exit = match args.first().map(String::as_str) {
        Some("register") => register::run(&args, format).await,
        Some("dial") => dial::run(&args, format).await,
        Some("answer") => answer::run(&args, format).await,
        Some("devices") => device::list(&args, format),
        Some("load") => load::run(&args, format).await,
        Some("load-responder") => load_responder::run(&args, format).await,
        // Not async, and deliberately so: listing what can be called reads a file and opens no
        // socket. The registrar and local-link sources are separate stories.
        Some("peers") => peers::run(&args, format),
        Some("scenario") => scenario::run(&args).await,
        Some("version" | "--version" | "-V") => {
            println!("sipx {}", env!("CARGO_PKG_VERSION"));
            Exit::Success
        }
        Some("help" | "--help" | "-h") | None => {
            print!("{USAGE}");
            Exit::Success
        }
        Some(unknown) => {
            eprint!("{USAGE}");
            output::fail(format, Exit::Usage, &format!("unknown command: {unknown}"))
        }
    };

    ExitCode::from(u8::try_from(exit.code()).unwrap_or(1))
}

/// How many times `-v` was asked for.
///
/// Counted by `v` **letter**, over every argument that is a `-` followed by nothing else, so the two
/// spellings of one request agree: `-vv` is `-v -v`, and `-vvv` is `-v -v -v`. It counted *arguments*
/// beginning with `-v` until `X-57` — which made `-vv` a single match, capped at INFO by [`level`],
/// while `USAGE` documented it as DEBUG and nothing on a call's path logged at INFO at all. The
/// documented flag was therefore accepted, and silent, and only the undocumented `-v -v` reached the
/// level both this help text and that function meant by it.
///
/// Only a cluster of `v`s counts, where the old prefix match counted anything starting `-v`. `-V` is
/// the version flag and there is no `--verbose`.
fn verbosity(args: &[String]) -> usize {
    args.iter()
        .filter_map(|arg| arg.strip_prefix('-'))
        .filter(|cluster| !cluster.is_empty() && cluster.bytes().all(|letter| letter == b'v'))
        .map(str::len)
        .sum()
}

/// The most detail that many `v`s asks for.
///
/// One level per `v` from WARN, **stopping at DEBUG**, so `-vvv` and beyond are `-vv`. DEBUG is the
/// last rung the workspace has anything standing on — it contains no `trace!` call — and mapping a
/// third `v` to TRACE would document a level whose output is identical to `-vv`'s, which is the defect
/// `X-57` is about and not a fix for it. Saturating rather than refusing is deliberate too: an
/// operator who holds the key down is asking for everything there is, and a usage error about a flag
/// that means "more" would be a strange way to answer that.
fn level(verbosity: usize) -> tracing::Level {
    match verbosity {
        0 => tracing::Level::WARN,
        1 => tracing::Level::INFO,
        _ => tracing::Level::DEBUG,
    }
}

fn init_logging(verbosity: usize) {
    let _ = tracing_subscriber::fmt()
        .with_max_level(level(verbosity))
        .with_writer(std::io::stderr)
        .try_init();
}

/// Whether these arguments ask for help.
///
/// Read from the raw arguments, and before they are validated: `--help` is a request for
/// documentation, so refusing to print it because some *other* flag on the line is malformed
/// answers a question nobody asked.
#[must_use]
pub(crate) fn wants_help(raw: &[String]) -> bool {
    raw.iter().any(|arg| arg == "--help" || arg == "-h")
}

/// A command's arguments, or the exit it should return instead of running.
///
/// Every command opens the same way — answer `--help`, then refuse an argument list that cannot be
/// honoured — and it lives here rather than four times over because the *order* is the part worth
/// getting right once. Help is answered first so that `sipx dial --help --play` documents the
/// command instead of complaining about `--play`.
///
/// `Err` carries the exit to return, which is `Exit::Success` when help was printed: from the
/// caller's side both arms are "stop here", and distinguishing them would only give the commands
/// the chance to disagree about it.
pub(crate) fn arguments<'a>(
    raw: &'a [String],
    help: &str,
    format: Format,
) -> Result<Args<'a>, Exit> {
    if wants_help(raw) {
        print!("{help}");
        return Err(Exit::Success);
    }
    Args::new(raw).map_err(|message| output::fail(format, Exit::Usage, &message))
}

/// Turn a signalling capture on if `--capture <path>` asked for one.
///
/// `docs/specs/sip-transport.md` §13. One function rather than three copies because all three
/// commands that bind an endpoint want exactly the same thing, and because the *reason* below should
/// be written once.
///
/// **There is deliberately no flag to turn redaction off.** The library allows it — a lab capture
/// against a test registrar has no secret worth removing, and redaction would hide the digest bug the
/// capture was taken to find — but exposing that here would put "ship the credentials" one word away
/// from someone debugging an incident at 3am, which is the moment they are least able to weigh it.
/// A caller who genuinely needs an unredacted capture is writing code, not typing a flag.
///
/// Nothing is validated about the path here. `bind` fails with `Error::Capture` naming it, which is a
/// better error than anything this could produce by guessing, and checking twice would leave two
/// answers to keep in step.
fn apply_capture(args: &Args<'_>, config: &mut sipx_transport::Config) {
    if let Some(path) = args.value("capture") {
        // `Args::new` has already refused an empty value (`S-30`), so a path here is a real one and
        // `None` means the flag was absent.
        config.capture = Some(sipx_transport::CaptureConfig::new(path));
    }
}

/// Record what the far end sends, for at most `within`, stopping once it has been quiet for `idle`.
///
/// Two questions, two bounds. That is the whole point of this function existing, and `X-40` is what
/// established it needed to (`docs/stories/X-40-*.md` carries the measurement).
///
/// How long the stream takes to **start** is a property of the machine — two jitter buffers filling,
/// a scheduler that is busy elsewhere — so it is bounded only by the call's own duration. How long a
/// gap means the far end has **stopped talking** is a property of the conversation, so it keeps a
/// short window. Both commands used to spend one 500 ms window on both questions, via
/// `MediaSession::record_until_idle(500ms)`, and under load the first frame arrived after it: the
/// loop ended before its first iteration and a call that carried audio was written out as a valid WAV
/// with **zero** samples. `MediaSession::record_at_least`'s "Why this exists (`X-28`)" predicted this
/// exactly — "a recording of zero samples — not a degraded one" — and this is the same cure applied
/// to the callers that were left on the old primitive. Widening the one window would only have moved
/// the cliff, which is why there are two.
///
/// This lives here rather than in `answer` or `dial` because both need it and it is the kind of
/// arithmetic that drifts once it is written twice — `answer` was the reported failure and `dial`
/// records with the same code.
///
/// **Whatever arrived is returned, including nothing.** A recording cut short by `within` is still
/// the audio the call carried, and both callers used to reach it through
/// `timeout(duration, ..).unwrap_or_default()`, which replaced a partial recording with silence at
/// the moment the cap fired — losing the whole thing to save none of it. The bound is enforced in
/// here so that there is no timed-out future left for a caller to unwrap.
/// Takes the `Call` rather than the `MediaSession` only because `sipx-media` is not a direct
/// dependency of this crate and the session type is not re-exported; the audio is all this touches.
async fn record(
    call: &sipx_call::Call,
    within: std::time::Duration,
    idle: std::time::Duration,
) -> Vec<i16> {
    record_media(call.media(), within, idle).await
}

/// The recording lifecycle shared by confirmed and reliable-provisional media sessions.
///
/// `Dialing` intentionally is not a `Call`: a final answer has not arrived. Keeping this helper
/// on the media session lets the diagnostic phone capture that phase without manufacturing a
/// confirmed-call handle or duplicating the two-bound recording rule above.
async fn record_media(
    media: &sipx_media::MediaSession,
    within: std::time::Duration,
    idle: std::time::Duration,
) -> Vec<i16> {
    let deadline = tokio::time::Instant::now() + within;
    let mut recorded = Vec::new();

    // The stream starting. Bounded by the call and by nothing tighter, because there is no gap to
    // measure yet — a far end that has not spoken is not a far end that has stopped.
    match tokio::time::timeout_at(deadline, media.recv()).await {
        Ok(Some(frame)) => recorded.extend_from_slice(&frame),
        // Nothing ever came, or the call ended first. Either way there is no stream whose end to
        // wait for, and an empty recording is the honest answer.
        Ok(None) | Err(_) => return recorded,
    }

    // The rest of it. Now that audio is flowing, a gap of `idle` does mean the far end has finished,
    // and the call's deadline still caps a peer that never stops talking.
    loop {
        let next = tokio::time::Instant::now() + idle;
        match tokio::time::timeout_at(next.min(deadline), media.recv()).await {
            Ok(Some(frame)) => recorded.extend_from_slice(&frame),
            // The far end went quiet, the call ended, or its time is up. All three mean this is
            // everything there is — and it is kept.
            Ok(None) | Err(_) => return recorded,
        }
    }
}

/// How long a gap in the audio means the far end has stopped talking.
///
/// Shared by both commands so the two cannot come to disagree about it.
const RECORD_IDLE: std::time::Duration = std::time::Duration::from_millis(500);

/// How long a silence means the caller has stopped dialling (`M-34`).
///
/// The value `answer` has always used, and it now does only this one job: it used to be the wait
/// for the *first* digit as well, so a caller who took longer than this to press anything had no
/// digits reported at all. That bound is the call's own duration now, the same way
/// [`record`] bounds the first frame of the recording.
const DIGIT_GAP: std::time::Duration = std::time::Duration::from_millis(800);

/// Shared argument parsing.
///
/// Deliberately small rather than a dependency: sipx needs flags and one positional, and a
/// parser for that is smaller than the code to configure a general one.
///
/// **Holding an `Args` means every valued flag on the line was given a value.** That invariant is
/// the reason the constructor is fallible. `value` used to answer `None` both for "the flag was
/// last, so nothing followed it" and for "the flag was absent" — one answer for two different
/// facts — so every caller took its absent-branch and the command ran on a default nobody typed:
/// `sipx register sip:alice@example.com --outbound --instance` exited 0 having generated an
/// instance URN that was never asked for (`S-30`). Establishing it once here rather than at each
/// call site is what stops the next flag from rediscovering it: a caller cannot forget a check it
/// does not have to make, and `None` from `value` now means absent and nothing else.
#[derive(Debug)]
pub(crate) struct Args<'a> {
    raw: &'a [String],
}

impl<'a> Args<'a> {
    /// Wrap the raw arguments, refusing invalid valued and numeric flags.
    ///
    /// Both ways a value goes missing are refused: nothing following the flag at all, and an empty
    /// value in either form (`--target=` or `--target ""`).
    ///
    /// The empty value is refused for *every* flag rather than for some. Nothing in `VALUED_FLAGS`
    /// has a meaningful empty value — not a password, not a path, not an address, not a count of
    /// seconds — and omitting the flag is already how a caller asks for the default, so an empty
    /// value can only be an accident. It is a common one: an unset shell variable expands to
    /// exactly this, which is how `--target "$ADDR"` arrives here. A per-flag exception list would
    /// be a second registry to hold in step with this one, for no case that wants it.
    ///
    /// The guarantee reaches only as far as `VALUED_FLAGS`, which is what
    /// `every_valued_flag_in_the_help_text_is_registered` exists to keep complete.
    pub(crate) fn new(raw: &'a [String]) -> Result<Self, String> {
        for (index, arg) in raw.iter().enumerate() {
            let Some(body) = arg.strip_prefix("--") else {
                continue;
            };
            // `--flag=value` carries its own value, even when that value is empty; `--flag value`
            // takes the next argument, if the caller left one there.
            let (name, given) = match body.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (body, raw.get(index + 1).map(String::as_str)),
            };
            let flag = format!("--{name}");
            if !VALUED_FLAGS.contains(&flag.as_str()) {
                continue;
            }
            match given {
                None => {
                    return Err(format!(
                        "{flag} takes a value and nothing followed it. A flag in final position is \
                         not an absent one — reading it as absent would run this command on a \
                         default that was not asked for"
                    ));
                }
                Some("") => {
                    return Err(format!(
                        "{flag} takes a value and was given an empty one. No flag here has a \
                         meaningful empty value, and an unset shell variable expands to exactly \
                         this, so it is refused rather than read as absent"
                    ));
                }
                Some(_) => {}
            }
            if NUMERIC_FLAGS.contains(&flag.as_str()) {
                let raw_value = given.unwrap_or_default();
                let value = raw_value.parse::<u64>().map_err(|_| {
                    format!(
                        "{flag} must be a whole number of seconds from 0 through {}, not \
                         {raw_value:?}",
                        u32::MAX
                    )
                })?;
                if value > u64::from(u32::MAX) {
                    return Err(format!(
                        "{flag} must be a whole number of seconds from 0 through {}, not \
                         {raw_value:?}",
                        u32::MAX
                    ));
                }
            }
        }
        Ok(Self { raw })
    }

    /// The value of `--name`, if given.
    ///
    /// `None` means the flag was absent. It cannot mean "given with no value" — `new` refused that
    /// argument list — so a caller may take its absent-branch on `None` without checking twice.
    #[must_use]
    pub(crate) fn value(&self, name: &str) -> Option<&'a str> {
        let flag = format!("--{name}");
        let mut iter = self.raw.iter();
        while let Some(arg) = iter.next() {
            if arg == &flag {
                return iter.next().map(String::as_str);
            }
            if let Some(rest) = arg.strip_prefix(&format!("{flag}=")) {
                return Some(rest);
            }
        }
        None
    }

    /// Every value of a repeatable option, in command-line order.
    ///
    /// The constructor has already established that each occurrence has a non-empty value.
    /// Keeping the order is load-bearing for `--codec`, where order is preference.
    pub(crate) fn values(&self, name: &str) -> impl Iterator<Item = &'a str> + '_ {
        let flag = format!("--{name}");
        self.raw.iter().enumerate().filter_map(move |(index, arg)| {
            if arg == &flag {
                return self.raw.get(index + 1).map(String::as_str);
            }
            arg.strip_prefix(&format!("{flag}="))
        })
    }

    /// Whether `--name` is present.
    #[must_use]
    pub(crate) fn flag(&self, name: &str) -> bool {
        let flag = format!("--{name}");
        self.raw.iter().any(|arg| arg == &flag)
    }

    /// The first argument that is not a flag or a flag's value.
    #[must_use]
    pub(crate) fn positional(&self) -> Option<&'a str> {
        let mut skip_next = false;
        for (index, arg) in self.raw.iter().enumerate() {
            if index == 0 {
                continue; // the subcommand
            }
            if skip_next {
                skip_next = false;
                continue;
            }
            if arg.starts_with('-') {
                // A flag with a separate value consumes the next argument.
                skip_next = !arg.contains('=') && VALUED_FLAGS.iter().any(|f| arg == f);
                continue;
            }
            return Some(arg);
        }
        None
    }

    /// A numeric option already validated by [`Self::new`].
    #[must_use]
    pub(crate) fn number(&self, name: &str) -> Option<u64> {
        self.value(name)?.parse().ok()
    }
}

/// Flags that take a separate value, so positional detection can skip past them.
///
/// A flag missing from this list has its *value* read as the positional argument, which turns
/// `sipx dial --timeout 30 sip:bob@host` into an attempt to call "30". There is a test below
/// asserting every flag the help text documents appears here, because the failure is silent
/// and the list is easy to forget.
const VALUED_FLAGS: &[&str] = &[
    "--password",
    "--play",
    "--record",
    "--duration",
    "--timeout",
    "--wait",
    "--dtmf",
    "--from",
    "--expires",
    "--local",
    "--advertise",
    "--target",
    "--book",
    "--instance",
    "--push-provider",
    "--push-prid",
    "--push-param",
    "--capture",
    "--counters",
    "--transport",
    "--tls-server-name",
    "--tls-ca",
    "--tls-cert",
    "--tls-key",
    "--profile",
    "--codec",
    "--media-security",
    "--ice",
    "--stun-server",
    "--audio-input",
    "--audio-output",
    "--header",
    "--rate",
    "--concurrency",
    "--calls",
    "--seed",
    "--call-duration",
    "--max-active",
    "--cleanup",
    "--dialog-duration",
    "--provisional-percent",
    "--answer-percent",
    "--reject-status",
    "--mode",
];

/// Flags whose values are whole seconds in the inclusive range `0..=u32::MAX`.
///
/// The upper bound is a command contract rather than an incidental integer width: every supported
/// platform and every deadline calculation can represent it, while a larger input is almost
/// certainly a shell/configuration mistake. Zero is meaningful only where a command gives it a
/// defined behavior:
///
/// - `--duration 0` ends an established call immediately;
/// - `--timeout 0` delegates to the transaction layer's own expiry;
/// - `--wait 0` checks once and returns immediately when no call is already queued;
/// - `--expires 0` asks the registrar to remove the binding.
///
/// `load-responder` refuses zero for `--cleanup`, `--dialog-duration`, and its admission-duration
/// use of `--duration`; sharing lexical validation does not make their semantic constraints equal.
///
/// `every_seconds_flag_is_registered_as_numeric` derives the documented `<S>` flags from the help
/// text and holds it against this list, so a future numeric flag cannot silently bypass validation.
const NUMERIC_FLAGS: &[&str] = &[
    "--duration",
    "--timeout",
    "--wait",
    "--expires",
    "--call-duration",
    "--cleanup",
    "--dialog-duration",
];

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    /// `Args` over an argument list that is expected to be well formed.
    fn parsed(raw: &[String]) -> Args<'_> {
        Args::new(raw).expect("a well formed argument list")
    }

    /// `-vv` is `-v -v` and `-vvv` is `-v -v -v` (`X-57`). Counting arguments rather than letters
    /// made the documented spelling a level quieter than the undocumented one, which is the whole
    /// defect: `-vv` matched once.
    #[test]
    fn a_repeated_v_is_counted_by_letter_so_the_spellings_agree() {
        assert_eq!(verbosity(&args(&["dial", "sip:a@b"])), 0);
        assert_eq!(verbosity(&args(&["dial", "-v"])), 1);
        assert_eq!(verbosity(&args(&["dial", "-vv"])), 2);
        assert_eq!(verbosity(&args(&["dial", "-v", "-v"])), 2);
        assert_eq!(verbosity(&args(&["dial", "-v", "-vv"])), 3);
        assert_eq!(
            verbosity(&args(&["dial", "-vvv"])),
            verbosity(&args(&["dial", "-v", "-v", "-v"])),
            "the two spellings of three must not disagree"
        );
    }

    /// Nothing else is this flag. The old prefix match counted every argument beginning `-v`, so a
    /// value or a flag that merely started that way raised the level.
    #[test]
    fn only_a_cluster_of_vs_is_verbosity() {
        assert_eq!(verbosity(&args(&["version", "-V"])), 0);
        assert_eq!(verbosity(&args(&["dial", "--verbose"])), 0);
        assert_eq!(verbosity(&args(&["dial", "-vx"])), 0);
        assert_eq!(verbosity(&args(&["dial", "-"])), 0);
    }

    /// The ladder `USAGE` documents, top included: more `v`s than there are levels is the top level
    /// rather than a refusal, and the top is DEBUG because the workspace has no `trace!` to reach.
    #[test]
    fn the_ladder_climbs_one_level_per_v_and_stops_at_debug() {
        assert_eq!(level(0), tracing::Level::WARN);
        assert_eq!(level(1), tracing::Level::INFO);
        assert_eq!(level(2), tracing::Level::DEBUG);
        assert_eq!(level(3), tracing::Level::DEBUG);
        assert_eq!(level(9), tracing::Level::DEBUG);
    }

    #[test]
    fn a_flag_value_is_read_in_either_form() {
        let raw = args(&["dial", "--password", "secret", "sip:a@b"]);
        assert_eq!(parsed(&raw).value("password"), Some("secret"));

        let raw = args(&["dial", "--password=secret", "sip:a@b"]);
        assert_eq!(parsed(&raw).value("password"), Some("secret"));
    }

    #[test]
    fn a_missing_flag_reads_as_absent() {
        let raw = args(&["dial", "sip:a@b"]);
        assert_eq!(parsed(&raw).value("password"), None);
        assert!(!parsed(&raw).flag("json"));
    }

    /// A flag's value must not be mistaken for the positional argument. Getting this wrong
    /// makes `sipx dial --password secret sip:a@b` try to call "secret".
    #[test]
    fn a_flag_value_is_not_mistaken_for_the_positional() {
        let raw = args(&["dial", "--password", "secret", "sip:bob@example.com"]);
        assert_eq!(parsed(&raw).positional(), Some("sip:bob@example.com"));

        let raw = args(&["dial", "sip:bob@example.com", "--password", "secret"]);
        assert_eq!(parsed(&raw).positional(), Some("sip:bob@example.com"));

        let raw = args(&["dial", "--json", "sip:bob@example.com"]);
        assert_eq!(parsed(&raw).positional(), Some("sip:bob@example.com"));
    }

    /// Every flag that takes a value must be listed, or its value is read as the positional
    /// argument. The failure is silent — the command tries to call "30" — so this checks the
    /// list against the help text of every command rather than trusting anyone to remember.
    #[test]
    fn every_valued_flag_in_the_help_text_is_registered() {
        let help = format!(
            "{}{}{}{}{}{}{}{}",
            USAGE,
            crate::register::HELP,
            crate::dial::HELP,
            crate::answer::HELP,
            crate::peers::HELP,
            crate::load::HELP,
            crate::load_responder::HELP,
            crate::scenario::HELP
        );

        // A documented flag takes a value if its help line shows a placeholder after it.
        let mut documented = Vec::new();
        for line in help.lines() {
            let trimmed = line.trim_start();
            let Some(rest) = trimmed.strip_prefix("--") else {
                continue;
            };
            let Some((flag, tail)) = rest.split_once(char::is_whitespace) else {
                continue;
            };
            if tail.trim_start().starts_with('<') {
                documented.push(format!("--{flag}"));
            }
        }
        assert!(
            !documented.is_empty(),
            "the help text lists no valued flags"
        );

        for flag in documented {
            assert!(
                VALUED_FLAGS.contains(&flag.as_str()),
                "{flag} takes a value but is missing from VALUED_FLAGS, so its value would be \
                 read as the positional argument"
            );
        }
    }

    /// Every flag that takes a value refuses to be given none — checked over `VALUED_FLAGS`
    /// itself, so a flag added to the registry later is covered without anyone adding a case.
    ///
    /// Deriving it from the registry rather than from the four flags that exposed the defect is
    /// the point: the hole was in `Args::value`, so it belonged to every flag at once, and an
    /// enumeration here would have to be remembered. `tests/cli.rs` asserts the same rule through
    /// the binary, where the exit code lives; this one asserts it over the whole list.
    #[test]
    fn every_valued_flag_is_refused_when_it_is_given_no_value() {
        for flag in VALUED_FLAGS.iter().copied() {
            // Nothing follows the flag at all.
            let raw = args(&["register", flag]);
            let error =
                Args::new(&raw).expect_err("a valued flag in final position was given no value");
            assert!(
                error.contains(flag),
                "the refusal must name {flag}: {error}"
            );

            // An empty value, in both forms a shell can hand one over.
            let joined = format!("{flag}=");
            for items in [
                &["register", joined.as_str(), "sip:a@b.c"][..],
                &["register", flag, "", "sip:a@b.c"][..],
            ] {
                let raw = args(items);
                let error = Args::new(&raw).expect_err("an empty value is not a value");
                assert!(
                    error.contains(flag),
                    "the refusal must name {flag}: {error}"
                );
            }
        }
    }

    /// The refusal is about flags that take a value, and must not spread to the ones that do not:
    /// `--tcp` in final position is a complete argument, and `--json=` is not any flag's problem.
    #[test]
    fn a_valueless_flag_is_untouched_by_the_rule() {
        let raw = args(&["register", "sip:a@b.c", "--outbound", "--tcp"]);
        assert!(parsed(&raw).flag("tcp"));

        let raw = args(&["dial", "sip:a@b.c", "--json"]);
        assert!(parsed(&raw).flag("json"));

        // `--json=` is nothing this rule has an opinion about: the flag takes no value, so an
        // empty one is not a value gone missing.
        let raw = args(&["dial", "sip:a@b.c", "--json="]);
        assert!(Args::new(&raw).is_ok());
    }

    /// A positional argument may contain an `=` — a URI parameter is spelled with one — and that
    /// must not be mistaken for a flag being assigned an empty value.
    #[test]
    fn an_equals_in_the_positional_is_not_a_flag() {
        let raw = args(&["register", "sip:alice@example.com;transport=tcp"]);
        assert_eq!(
            parsed(&raw).positional(),
            Some("sip:alice@example.com;transport=tcp")
        );
    }

    /// The case the missing entry actually breaks.
    #[test]
    fn a_timeout_before_the_uri_does_not_become_the_uri() {
        let raw = args(&["dial", "--timeout", "30", "sip:bob@192.0.2.1:5060"]);
        assert_eq!(parsed(&raw).positional(), Some("sip:bob@192.0.2.1:5060"));
        assert_eq!(parsed(&raw).value("timeout"), Some("30"));

        let raw = args(&["answer", "--wait", "20", "--json"]);
        assert_eq!(
            parsed(&raw).positional(),
            None,
            "answer takes no positional"
        );
    }

    #[test]
    fn a_numeric_option_is_validated_before_it_can_be_read() {
        let raw = args(&["dial", "--duration", "30"]);
        assert_eq!(parsed(&raw).number("duration"), Some(30));

        let raw = args(&["dial", "--duration", "thirty"]);
        let error = Args::new(&raw).expect_err("a non-number is not an absent flag");
        assert!(error.contains("--duration"), "{error}");
        assert!(error.contains("whole number"), "{error}");
    }

    #[test]
    fn every_seconds_flag_is_registered_as_numeric() {
        let help = format!(
            "{}{}{}{}{}{}{}{}",
            USAGE,
            crate::register::HELP,
            crate::dial::HELP,
            crate::answer::HELP,
            crate::peers::HELP,
            crate::load::HELP,
            crate::load_responder::HELP,
            crate::scenario::HELP
        );
        let documented: Vec<String> = help
            .lines()
            .filter_map(|line| {
                let rest = line.trim_start().strip_prefix("--")?;
                let (flag, tail) = rest.split_once(char::is_whitespace)?;
                tail.trim_start()
                    .starts_with("<S>")
                    .then(|| format!("--{flag}"))
            })
            .collect();

        assert!(
            !documented.is_empty(),
            "the help documents no seconds flags"
        );
        for flag in &documented {
            assert!(
                NUMERIC_FLAGS.contains(&flag.as_str()),
                "{flag} is documented as seconds but is not validated as numeric"
            );
        }
        for flag in NUMERIC_FLAGS {
            assert!(
                documented.iter().any(|item| item == flag),
                "{flag} is validated as numeric but not documented with <S>"
            );
        }
    }

    #[test]
    fn numeric_boundaries_are_the_declared_ones_for_every_seconds_flag() {
        for flag in NUMERIC_FLAGS {
            for accepted in ["0", "4294967295"] {
                let raw = args(&["answer", flag, accepted]);
                assert!(Args::new(&raw).is_ok(), "{flag} must accept {accepted}");
            }
            for refused in ["-1", "4294967296", "18446744073709551616"] {
                let raw = args(&["answer", flag, refused]);
                let error = Args::new(&raw).expect_err("outside the declared numeric domain");
                assert!(error.contains(flag), "{error}");
            }
        }
    }
}
