//! `sipx dial`.

use std::net::IpAddr;
use std::time::Duration;

use sipx_audio::{Wav, read_wav, write_wav};
use sipx_call::Call;
use sipx_sip::Uri;
use sipx_transport::{Config as TransportConfig, Target, TransportKind, bind};

use crate::Args;
use crate::output::{Exit, Format, Report, fail};

pub(crate) const HELP: &str = "\
sipx dial — place a call

USAGE:
    sipx dial <URI> [OPTIONS]

ARGS:
    <URI>    Who to call, e.g. sip:bob@192.0.2.1:5060

OPTIONS:
    --play <FILE>     Play this WAV into the call (8 kHz 16-bit mono)
    --record <FILE>   Record the far end to this WAV
    --dtmf <DIGITS>   Send these digits once the call is up
    --duration <S>    Hang up after this many seconds once connected (default 30)
    --timeout <S>     Give up if the call is not answered in this many seconds (default 20).
                      0 waits as long as the transaction layer does, which is 32 seconds.
    --from <URI>      Our own address (default sip:sipx@<local>)
    --local <ADDR>    Local address to bind (default 0.0.0.0:0)
    --tcp             Use TCP rather than UDP
    --stats           Report call quality on exit: loss, jitter, round trip, MOS estimate
    --capture <FILE>  Record the signalling to this pcapng file (credentials redacted)
    --json            Report as JSON
";

pub(crate) async fn run(raw: &[String], format: Format) -> Exit {
    // Help, then any flag given no value — refused before the URI is even looked at, so a dropped
    // `--play` or `--record` cannot turn into a call that carries no audio (`S-30`).
    let args = match crate::arguments(raw, HELP, format) {
        Ok(args) => args,
        Err(exit) => return exit,
    };

    let Some(uri) = args.positional() else {
        eprint!("{HELP}");
        return fail(format, Exit::Usage, "a URI to call is required");
    };

    // Before anything else, and before the URI is parsed for an address: a scheme this CLI cannot
    // honour is refused rather than quietly downgraded.
    if let Some(why) = crate::insecure_scheme_refusal(uri) {
        return fail(format, Exit::Usage, &why);
    }

    // `--password` is a global valued flag, so `dial` *parses* it. It used to then throw it away:
    // a call challenged with 407 failed while the person who supplied credentials was told nothing
    // (`P-7`). Answering the challenge is not a CLI change — `sipx-call` has no credential type and
    // no 401/407 path at all — so the flag is refused rather than silently ignored. `S-28` is the
    // feature; until it exists, saying so is the only honest option.
    if args.value("password").is_some() {
        return fail(
            format,
            Exit::Usage,
            "--password is not supported by `dial`: a call cannot answer an authentication \
             challenge yet, so a password here would be silently ignored. `register` does \
             authenticate; see S-28 for the call-layer work",
        );
    }

    let transport = if args.flag("tcp") {
        TransportKind::Tcp
    } else {
        TransportKind::Udp
    };

    let Some(target_addr) = target_of(uri) else {
        return fail(
            format,
            Exit::Usage,
            &format!("{uri} must name an address and port, e.g. sip:bob@192.0.2.1:5060"),
        );
    };
    let target = Target::new(target_addr, transport);

    // Audio to play is read before the call is placed: failing after the far end has answered
    // means hanging up on someone for a mistake that was visible beforehand.
    let clip = match args.value("play").map(read_clip) {
        Some(Ok(clip)) => Some(clip),
        Some(Err(message)) => return fail(format, Exit::Usage, &message),
        None => None,
    };

    let local: std::net::SocketAddr = match args.value("local").unwrap_or("0.0.0.0:0").parse() {
        Ok(local) => local,
        Err(_) => return fail(format, Exit::Usage, "--local must be host:port"),
    };
    // Media has to advertise something reachable, and an unspecified address is not.
    let media_address: IpAddr = crate::advertise::reachable_ip(local, target_addr.ip());

    let mut config = TransportConfig::new(local);
    config.sent_by = media_address.to_string();
    crate::apply_capture(&args, &mut config);
    let (handle, _incoming) = match bind(config).await {
        Ok(bound) => bound,
        Err(error) => return fail(format, Exit::Failed, &format!("bind: {error}")),
    };

    let Ok(to) = Uri::parse(bytes::Bytes::from(uri.to_owned())) else {
        return fail(format, Exit::Usage, &format!("not a SIP URI: {uri}"));
    };
    let from = args
        .value("from")
        .map_or_else(|| format!("<sip:sipx@{media_address}>"), str::to_owned);

    // The bound is handed to the library rather than wrapped around it. Dropping the call
    // future partway through would leave the far end believing it is in a call, and only code
    // inside the exchange can send the CANCEL that stops it ringing.
    let attempt = match numeric(&args, "timeout", DEFAULT_TIMEOUT_SECS) {
        Ok(seconds) => Duration::from_secs(seconds),
        Err(message) => return fail(format, Exit::Usage, &message),
    };

    let mut options = sipx_call::DialOptions::new(from, media_address);
    if !attempt.is_zero() {
        options = options.with_timeout(attempt);
    }

    let started = std::time::Instant::now();
    let mut call = match sipx_call::dial(&handle, target, &to, &options).await {
        Ok(call) => call,
        Err(error) => return report_failure(format, &error),
    };

    let duration = match numeric(&args, "duration", 30) {
        Ok(seconds) => Duration::from_secs(seconds),
        Err(message) => return fail(format, Exit::Usage, &message),
    };
    let recorded = exchange(&mut call, clip.as_ref(), args.value("dtmf"), duration).await;

    let mut report = Report::new()
        .text("status", "answered")
        .text("peer", uri)
        .number(
            "duration_ms",
            i64::try_from(started.elapsed().as_millis()).unwrap_or(0),
        )
        .number(
            "samples_recorded",
            i64::try_from(recorded.len()).unwrap_or(0),
        )
        .boolean("heard_audio", !recorded.is_empty());

    if args.flag("stats") {
        report = with_quality(report, &call.media().quality().await);
    }

    if let Some(path) = args.value("record") {
        match write_clip(path, &recorded) {
            Ok(()) => report = report.text("recording", path),
            Err(message) => return fail(format, Exit::Failed, &message),
        }
    }

    let _ = call.hang_up().await;
    report.emit(format);
    Exit::Success
}

/// Play, send digits and record for the duration of the call.
async fn exchange(
    call: &mut Call,
    clip: Option<&Wav>,
    dtmf: Option<&str>,
    duration: Duration,
) -> Vec<i16> {
    let media = call.media();

    let playing = async {
        if let Some(clip) = clip {
            media.play(&clip.samples, 160).await;
        }
        if let Some(digits) = dtmf {
            // After the audio, so a menu hears the prompt before the keypress.
            call.send_digits(digits, Duration::from_millis(100)).await;
        }
    };

    // Recording stops when the far end goes quiet, or when the call's time is up — whichever
    // comes first, so a peer that never stops talking cannot hold the command open.
    let recording = media.record_until_idle(Duration::from_millis(500));

    let (_, recorded) = tokio::join!(
        tokio::time::timeout(duration, playing),
        tokio::time::timeout(duration, recording)
    );
    recorded.unwrap_or_default()
}

/// How long to wait for an answer.
///
/// Deliberately *under* 64·T1 — the transaction layer's own 32-second expiry — so that when a
/// call goes unanswered it is always this bound that fires. Setting them equal makes which one
/// wins a matter of scheduling, and the error a script reads changes between runs.
const DEFAULT_TIMEOUT_SECS: u64 = 20;

/// A numeric option, or a usage error naming what was wrong with it.
///
/// `Args::number` returns `None` for both "absent" and "not a number", and silently falling
/// back to the default for the second means `--timeout=3s` restores exactly the behaviour the
/// flag was added to avoid, with no diagnostic.
fn numeric(args: &Args<'_>, name: &str, default: u64) -> std::result::Result<u64, String> {
    match args.value(name) {
        None => Ok(default),
        Some(raw) => raw
            .parse()
            .map_err(|_| format!("--{name} must be a whole number of seconds, not {raw:?}")),
    }
}

fn report_failure(format: Format, error: &sipx_call::Error) -> Exit {
    let exit = match error {
        sipx_call::Error::Rejected { status, .. } => Exit::for_status(*status),
        sipx_call::Error::NoResponse | sipx_call::Error::Cancelled(_) => Exit::Timeout,
        _ => Exit::Failed,
    };
    fail(format, exit, &error.to_string())
}

/// Read a clip, insisting on the format the codec needs.
fn read_clip(path: &str) -> Result<Wav, String> {
    let file = std::fs::File::open(path).map_err(|error| format!("{path}: {error}"))?;
    let clip = read_wav(file).map_err(|error| format!("{path}: {error}"))?;
    if clip.sample_rate != 8000 {
        // Resampling is a real feature, not a silent one. Playing 44.1 kHz samples at 8 kHz
        // produces audio that is recognisably wrong rather than obviously broken.
        return Err(format!(
            "{path}: {} Hz; G.711 needs 8000 Hz — resample it first",
            clip.sample_rate
        ));
    }
    Ok(clip)
}

fn write_clip(path: &str, samples: &[i16]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|error| format!("{path}: {error}"))?;
    write_wav(file, &Wav::narrowband(samples.to_vec())).map_err(|error| format!("{path}: {error}"))
}

/// The address and port a URI names, if it names one directly.
pub(crate) fn target_of(uri: &str) -> Option<std::net::SocketAddr> {
    let rest = uri
        .strip_prefix("sip:")
        .or_else(|| uri.strip_prefix("sips:"))?;
    let host = rest.rsplit('@').next()?;
    let host = host.split(';').next()?;
    if let Ok(addr) = host.parse::<std::net::SocketAddr>() {
        return Some(addr);
    }
    host.parse::<IpAddr>()
        .ok()
        .map(|ip| std::net::SocketAddr::new(ip, 5060))
}

/// Add the call's quality to a report.
///
/// The round-trip time is added only when there is one. A peer that does not speak RTCP never
/// gives us the numbers to compute it, and reporting `0` there would say "instantaneous" —
/// which a script would believe.
fn with_quality(report: Report, quality: &sipx_rtp::Quality) -> Report {
    let report = report
        .decimal("loss", quality.loss, 4)
        .number("packets_lost", quality.cumulative_lost)
        .number(
            "jitter_ms",
            i64::try_from(quality.jitter.as_millis()).unwrap_or(i64::MAX),
        )
        // Two places. The E-model behind this is an estimate with simplified impairment terms,
        // and more digits would be inventing precision it does not have.
        .decimal("mos", quality.mos, 2);
    match quality.round_trip {
        Some(trip) => report.millis("round_trip_ms", trip),
        None => report,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    /// Both formats must carry the same facts — that is the rule the whole output module is
    /// built on, and a statistics block added to one and not the other is the easiest way to
    /// break it.
    #[test]
    fn stats_render_the_same_facts_in_both_formats() {
        let quality = sipx_rtp::Quality {
            loss: 0.0123,
            cumulative_lost: 7,
            jitter: std::time::Duration::from_millis(12),
            round_trip: Some(std::time::Duration::from_millis(84)),
            mos: 4.128_745_9,
        };
        let report = with_quality(Report::new(), &quality);

        let text = report.render(Format::Text);
        let json = report.render(Format::Json);
        for field in ["loss", "packets_lost", "jitter_ms", "mos", "round_trip_ms"] {
            assert!(text.contains(field), "text is missing {field}: {text}");
            assert!(json.contains(field), "json is missing {field}: {json}");
        }
    }

    /// A score printed to eight digits invites someone to compare two calls on the last one.
    /// The model behind it does not support that, so the output does not offer it.
    #[test]
    fn the_score_is_not_reported_to_more_precision_than_it_has() {
        let quality = sipx_rtp::Quality {
            loss: 0.0,
            cumulative_lost: 0,
            jitter: std::time::Duration::ZERO,
            round_trip: None,
            mos: 4.128_745_9,
        };
        let rendered = with_quality(Report::new(), &quality).render(Format::Json);
        assert!(rendered.contains("4.13"), "{rendered}");
        assert!(!rendered.contains("4.1287"), "{rendered}");
    }

    /// A peer that does not speak RTCP never gives us the numbers for a round trip. Reporting
    /// `0` would say "instantaneous", and a script would believe it; the field is absent.
    #[test]
    fn an_unmeasurable_round_trip_is_absent_rather_than_zero() {
        let quality = sipx_rtp::Quality {
            loss: 0.0,
            cumulative_lost: 0,
            jitter: std::time::Duration::ZERO,
            round_trip: None,
            mos: 4.4,
        };
        let rendered = with_quality(Report::new(), &quality).render(Format::Json);
        assert!(
            !rendered.contains("round_trip"),
            "an absent measurement must not be reported as zero: {rendered}"
        );
    }

    use super::*;

    #[test]
    fn a_uri_naming_an_address_and_port_is_the_target() {
        assert_eq!(
            target_of("sip:bob@192.0.2.1:5080").map(|a| a.to_string()),
            Some("192.0.2.1:5080".to_owned())
        );
    }

    #[test]
    fn a_uri_without_a_port_gets_the_default() {
        assert_eq!(
            target_of("sip:bob@192.0.2.1").map(|a| a.to_string()),
            Some("192.0.2.1:5060".to_owned())
        );
    }

    #[test]
    fn a_uri_with_no_user_still_yields_its_host() {
        assert_eq!(
            target_of("sip:192.0.2.1:5060").map(|a| a.to_string()),
            Some("192.0.2.1:5060".to_owned())
        );
    }

    #[test]
    fn uri_parameters_are_not_part_of_the_host() {
        assert_eq!(
            target_of("sip:bob@192.0.2.1:5060;transport=tcp").map(|a| a.to_string()),
            Some("192.0.2.1:5060".to_owned())
        );
    }

    #[test]
    fn a_name_is_not_a_target_this_command_can_use() {
        assert!(target_of("sip:bob@example.com").is_none());
        assert!(target_of("bob@192.0.2.1").is_none(), "no scheme");
    }

    /// `S-27`. Before this, `target_of` stripped `sips:` in the same `or_else` as `sip:`, so the
    /// address came back looking ordinary and the INVITE went out over UDP in the clear. The
    /// downgrade was invisible: the call connects and the audio flows.
    #[test]
    fn a_sips_uri_is_refused_rather_than_dialled_in_the_clear() {
        let why = crate::insecure_scheme_refusal("sips:bob@192.0.2.1")
            .expect("a sips: URI this CLI cannot honour must be refused");
        assert!(
            why.contains("TLS"),
            "the refusal has to name the missing capability, not call the URI malformed: {why}"
        );
        // The proof that the old behaviour was a silent downgrade rather than a parse failure:
        // the address still resolves perfectly well, which is exactly why nothing caught it.
        assert_eq!(
            target_of("sips:bob@192.0.2.1").map(|a| a.to_string()),
            Some("192.0.2.1:5060".to_owned()),
            "if this ever returns None, the refusal above is no longer what protects the caller"
        );
    }

    /// The behavioural half of `S-27`, and the one that actually matters: the *command* refuses,
    /// not just a helper. This needs no network and no peer, which is the point — the check happens
    /// before a transport is chosen or an address parsed, so there is no window in which a cleartext
    /// datagram could leave.
    /// The behavioural half of `S-27`, and the one that matters: the *command* refuses, not just a
    /// helper. It needs no network and no peer, which is the point — the refusal happens before a
    /// transport is chosen, so there is no window in which a cleartext datagram could leave.
    ///
    /// **The first argument must be the subcommand.** `Args::positional` skips index 0 as the
    /// subcommand name (`main.rs:137-139`), so a slice holding only the URI has *no* positional and
    /// this returns `Usage` for "a URI to call is required" instead — which passes whether or not the
    /// scheme is checked. That is how this test was first written, and it detected nothing.
    #[tokio::test]
    async fn the_dial_command_refuses_a_sips_uri_before_touching_the_network() {
        let exit = run(
            &["dial".to_owned(), "sips:bob@192.0.2.1".to_owned()],
            Format::Text,
        )
        .await;
        assert_eq!(
            exit.code(),
            Exit::Usage.code(),
            "dialling a sips: URI must be refused, not connected in the clear"
        );
    }

    /// `P-7`. `--password` is a global valued flag, so `dial` parsed it and dropped it: a 407 ended
    /// the call while the person who supplied credentials was told nothing. Refusing is honest
    /// because answering the challenge is a `sipx-call` feature that does not exist — that crate has
    /// no credential type and no 401/407 path (`S-28`).
    #[tokio::test]
    async fn the_dial_command_refuses_a_password_it_cannot_use() {
        let exit = run(
            &[
                "dial".to_owned(),
                "--password".to_owned(),
                "secret".to_owned(),
                "sip:bob@192.0.2.1".to_owned(),
            ],
            Format::Text,
        )
        .await;
        assert_eq!(
            exit.code(),
            Exit::Usage.code(),
            "a password dial cannot use must be refused, not accepted and dropped"
        );
    }

    #[test]
    fn a_plain_sip_uri_is_not_refused() {
        assert!(crate::insecure_scheme_refusal("sip:bob@192.0.2.1").is_none());
        assert!(
            crate::insecure_scheme_refusal("bob@192.0.2.1").is_none(),
            "a URI with no scheme is target_of's to reject, and it says something else"
        );
    }

    /// A clip at the wrong rate is refused by name. Playing 44.1 kHz samples at 8 kHz produces
    /// audio that is recognisably wrong rather than obviously broken, which is harder to
    /// diagnose than a refusal.
    #[test]
    fn a_clip_at_the_wrong_sample_rate_is_refused() {
        let dir = std::env::temp_dir().join(format!("sipx-dial-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let path = dir.join("wide.wav");

        let wide = Wav {
            sample_rate: 44_100,
            samples: vec![0; 100],
        };
        write_wav(std::fs::File::create(&path).expect("creates"), &wide).expect("writes");

        let error = read_clip(path.to_str().expect("a path")).expect_err("refused");
        assert!(error.contains("44100"), "{error}");
        assert!(error.contains("8000"), "{error}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_clip_at_the_right_rate_is_accepted() {
        let dir = std::env::temp_dir().join(format!("sipx-dial-ok-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let path = dir.join("narrow.wav");

        let clip = Wav::narrowband(vec![1, 2, 3, 4]);
        write_wav(std::fs::File::create(&path).expect("creates"), &clip).expect("writes");

        let read = read_clip(path.to_str().expect("a path")).expect("accepted");
        assert_eq!(read.samples, clip.samples);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_is_reported_with_its_path() {
        let error = read_clip("/nonexistent/path/to/clip.wav").expect_err("refused");
        assert!(error.contains("/nonexistent/path/to/clip.wav"), "{error}");
    }
}
