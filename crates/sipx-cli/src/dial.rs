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
    --json            Report as JSON
";

pub(crate) async fn run(raw: &[String], format: Format) -> Exit {
    let args = Args::new(raw);
    if args.flag("help") || raw.iter().any(|a| a == "-h") {
        print!("{HELP}");
        return Exit::Success;
    }

    let Some(uri) = args.positional() else {
        eprint!("{HELP}");
        return fail(format, Exit::Usage, "a URI to call is required");
    };

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
    let media_address: IpAddr = if local.ip().is_unspecified() {
        // Media has to advertise something reachable, and an unspecified address is not.
        if target_addr.ip().is_loopback() {
            "127.0.0.1".parse().unwrap_or(target_addr.ip())
        } else {
            local_address_towards(target_addr.ip())
        }
    } else {
        local.ip()
    };

    let mut config = TransportConfig::new(local);
    config.sent_by = media_address.to_string();
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

/// Which of our addresses faces a peer.
///
/// Asking the routing table by opening a UDP socket towards the peer — no packet is sent, but
/// the kernel picks the source address it would use, which is the one to advertise.
fn local_address_towards(peer: IpAddr) -> IpAddr {
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|socket| {
            socket.connect(std::net::SocketAddr::new(peer, 9))?;
            socket.local_addr()
        })
        .map_or_else(|_| "127.0.0.1".parse().unwrap_or(peer), |addr| addr.ip())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
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
