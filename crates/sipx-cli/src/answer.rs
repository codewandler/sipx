//! `sipx answer`.

use std::net::IpAddr;
use std::time::Duration;

use sipx_audio::{Wav, read_wav, write_wav};
use sipx_sip::{HeaderName, StatusCode};
use sipx_transport::{Config as TransportConfig, bind};

use crate::Args;
use crate::output::{Exit, Format, Report, fail};

const HELP: &str = "\
sipx answer — wait for a call and answer it

USAGE:
    sipx answer [OPTIONS]

OPTIONS:
    --play <FILE>     Play this WAV to the caller (8 kHz 16-bit mono)
    --record <FILE>   Record the caller to this WAV
    --duration <S>    Hang up after this many seconds (default 30)
    --wait <S>        Give up if no call arrives within this many seconds (default 60)
    --local <ADDR>    Local address to bind (default 0.0.0.0:5060)
    --reject          Answer 603 Decline instead
    --busy            Answer 486 Busy Here instead
    --once            Exit after one call (default; kept for clarity in scripts)
    --json            Report as JSON
";

pub(crate) async fn run(raw: &[String], format: Format) -> Exit {
    let args = Args::new(raw);
    if args.flag("help") || raw.iter().any(|a| a == "-h") {
        print!("{HELP}");
        return Exit::Success;
    }

    let clip = match args.value("play").map(read_clip) {
        Some(Ok(clip)) => Some(clip),
        Some(Err(message)) => return fail(format, Exit::Usage, &message),
        None => None,
    };

    let local: std::net::SocketAddr = match args.value("local").unwrap_or("0.0.0.0:5060").parse() {
        Ok(local) => local,
        Err(_) => return fail(format, Exit::Usage, "--local must be host:port"),
    };

    let mut config = TransportConfig::new(local);
    if local.ip().is_unspecified() {
        "127.0.0.1".clone_into(&mut config.sent_by);
    }
    let (handle, mut incoming) = match bind(config).await {
        Ok(bound) => bound,
        Err(error) => return fail(format, Exit::Failed, &format!("bind: {error}")),
    };

    // Announce the port before waiting, so a script that started this in the background knows
    // where to call without guessing or racing.
    Report::new()
        .text("status", "listening")
        .text("address", handle.local_addr().to_string())
        .emit(format);

    let wait = Duration::from_secs(args.number("wait").unwrap_or(60));
    let Ok(Some(request)) = tokio::time::timeout(wait, incoming.recv()).await else {
        return fail(format, Exit::Timeout, "no call arrived");
    };

    let caller = request
        .request
        .headers
        .value(&HeaderName::From)
        .map(|value| String::from_utf8_lossy(&value).into_owned())
        .unwrap_or_default();

    if args.flag("reject") || args.flag("busy") {
        return refuse(&handle, &request, &caller, args.flag("busy"), format).await;
    }

    let media_address: IpAddr = if local.ip().is_unspecified() {
        // Answer on the address the caller reached us at, which is the one they can hear.
        request.source.ip()
    } else {
        local.ip()
    };

    let started = std::time::Instant::now();
    let mut call = match sipx_call::answer(&handle, &request, media_address).await {
        Ok(call) => call,
        Err(error) => return fail(format, Exit::Failed, &error.to_string()),
    };

    let duration = Duration::from_secs(args.number("duration").unwrap_or(30));
    let media = call.media();

    let ((), recorded, digits) = tokio::join!(
        async {
            if let Some(clip) = &clip {
                let _ = tokio::time::timeout(duration, media.play(&clip.samples, 160)).await;
            }
        },
        tokio::time::timeout(
            duration,
            media.record_until_idle(Duration::from_millis(500))
        ),
        tokio::time::timeout(duration, media.collect_digits(Duration::from_millis(800))),
    );

    let recorded = recorded.unwrap_or_default();
    let digits = digits.unwrap_or_default();

    let mut report = Report::new()
        .text("status", "answered")
        .text("caller", caller)
        .number(
            "duration_ms",
            i64::try_from(started.elapsed().as_millis()).unwrap_or(0),
        )
        .number(
            "samples_recorded",
            i64::try_from(recorded.len()).unwrap_or(0),
        )
        .boolean("heard_audio", !recorded.is_empty());

    if !digits.is_empty() {
        report = report.text("dtmf", digits);
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

/// Answer with a refusal rather than taking the call.
async fn refuse(
    handle: &sipx_transport::Handle,
    request: &sipx_transport::Incoming,
    caller: &str,
    busy: bool,
    format: Format,
) -> Exit {
    let (code, reason) = if busy {
        (486, "Busy Here")
    } else {
        (603, "Decline")
    };
    let Some(status) = StatusCode::new(code) else {
        return fail(format, Exit::Failed, "bad status");
    };
    let response =
        match sipx_sip::build::ResponseBuilder::to_request(&request.request, status, reason) {
            Ok(builder) => builder.build(),
            Err(error) => return fail(format, Exit::Failed, &error.to_string()),
        };
    let _ = handle.respond(&request.key, response).await;

    Report::new()
        .text("status", "refused")
        .text("caller", caller)
        .number("code", i64::from(code))
        .emit(format);
    Exit::Success
}

fn read_clip(path: &str) -> Result<Wav, String> {
    let file = std::fs::File::open(path).map_err(|error| format!("{path}: {error}"))?;
    let clip = read_wav(file).map_err(|error| format!("{path}: {error}"))?;
    if clip.sample_rate != 8000 {
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
