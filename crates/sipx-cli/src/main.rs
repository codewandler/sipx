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
mod cli;
mod counters;
mod destination;
mod device;
mod dial;
mod header;
mod load;
mod load_responder;
mod load_responder_readiness;
mod media;
mod output;
mod peers;
mod register;
mod scenario;
mod signalling;

use std::process::ExitCode;

use clap::error::ErrorKind;
use cli::{Cli, Command};
use output::{Exit, Format};

#[tokio::main]
async fn main() -> ExitCode {
    let cli = match Cli::parse_process() {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            print!("{error}");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            let exit = match Cli::requested_format() {
                Format::Json => output::fail(Format::Json, Exit::Usage, &error.to_string()),
                Format::Text => {
                    eprint!("{error}");
                    Exit::Usage
                }
            };
            return ExitCode::from(u8::try_from(exit.code()).unwrap_or(1));
        }
    };
    let format = if cli.json { Format::Json } else { Format::Text };

    // Logging goes to stderr. One stray line on stdout turns valid JSON into a parse error at
    // the far end of a pipe, where the cause is invisible.
    init_logging(usize::from(cli.verbose));

    let exit = match cli.command {
        Some(Command::Register(options)) => register::run(options, format).await,
        Some(Command::Dial(options)) => Box::pin(dial::run(options, format)).await,
        Some(Command::Answer(options)) => Box::pin(answer::run(options, format)).await,
        Some(Command::Devices(options)) => device::list(options, format),
        Some(Command::Load(options)) => load::run(options, format).await,
        Some(Command::LoadResponder(options)) => load_responder::run(options, format).await,
        Some(Command::Peers(options)) => peers::run(options, format).await,
        Some(Command::Scenario(options)) => scenario::run(options).await,
        Some(Command::Version(_)) => {
            println!("sipx {}", env!("CARGO_PKG_VERSION"));
            Exit::Success
        }
        None => {
            print!("{}", Cli::root_help());
            Exit::Success
        }
    };

    ExitCode::from(u8::try_from(exit.code()).unwrap_or(1))
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
fn apply_capture(options: &cli::CaptureOptions, config: &mut sipx_transport::Config) {
    if let Some(path) = options.capture.as_deref() {
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
/// [`record_media`] bounds the first frame of the recording.
const DIGIT_GAP: std::time::Duration = std::time::Duration::from_millis(800);

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn verbosity_stops_at_debug() {
        assert_eq!(level(0), tracing::Level::WARN);
        assert_eq!(level(1), tracing::Level::INFO);
        assert_eq!(level(2), tracing::Level::DEBUG);
        assert_eq!(level(9), tracing::Level::DEBUG);
    }
}
