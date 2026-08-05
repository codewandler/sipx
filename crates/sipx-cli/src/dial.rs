//! `sipx dial`.

use std::time::Duration;

use sipx_audio::{Wav, read_wav, write_wav};
use sipx_call::{Call, Credentials};
use sipx_sip::{Address, Host, Uri};
use sipx_transport::{Config as TransportConfig, TransportKind, bind};

use crate::cli::DialOptions;
use crate::output::{Exit, Format, Report, fail};

#[allow(
    clippy::too_many_lines,
    reason = "the command lifecycle is kept in execution order so validation-before-I/O remains auditable"
)]
pub(crate) async fn run(options: DialOptions, format: Format) -> Exit {
    let uri = options.uri.as_str();

    let Ok(to) = Uri::parse(bytes::Bytes::from(uri.to_owned())) else {
        return fail(format, Exit::Usage, &format!("not a SIP URI: {uri}"));
    };
    let transport = match crate::signalling::Selection::from_options(
        &options.signalling,
        to.scheme().is_secure(),
    ) {
        Ok(transport) => transport,
        Err(message) => return fail(format, Exit::Usage, &message),
    };
    let media = match crate::media::Selection::from_options(
        &options.media,
        transport.kind(),
        options.early_media,
    ) {
        Ok(media) => media,
        Err(message) => return fail(format, Exit::Usage, &message),
    };
    let audio = match crate::device::Selection::from_options(&options.audio) {
        Ok(audio) => audio,
        Err(message) => return fail(format, Exit::Usage, &message),
    };
    let headers = match crate::header::from_options(&options.headers) {
        Ok(headers) => headers,
        Err(message) => return fail(format, Exit::Usage, &message),
    };

    let password = options.password.clone();

    let Some((target_addr, server_name)) = target_of(&to, transport.kind()) else {
        return fail(
            format,
            Exit::Usage,
            &format!("{uri} must name an address and port, e.g. sip:bob@192.0.2.1:5060"),
        );
    };
    let target = match transport.target(&options.signalling, target_addr, &server_name) {
        Ok(target) => target,
        Err(message) => return fail(format, Exit::Usage, &message),
    };
    let negotiated_transport = target.transport;

    // Audio to play is read before the call is placed: failing after the far end has answered
    // means hanging up on someone for a mistake that was visible beforehand.
    let clip = match audio.wav_input().map(read_clip) {
        Some(Ok(clip)) => Some(clip),
        Some(Err(message)) => return fail(format, Exit::Usage, &message),
        None => None,
    };
    let mut devices = match audio.open() {
        Ok(devices) => devices,
        Err(message) => return fail(format, Exit::Failed, &message),
    };

    let local = options.local;
    // Media has to advertise something reachable, and an unspecified address is not.
    let advertised = match options.advertise {
        Some(address) if !address.is_unspecified() => Some(address),
        Some(_) => {
            return fail(
                format,
                Exit::Usage,
                "--advertise must be a non-unspecified IP",
            );
        }
        None => None,
    };
    let media_addresses = crate::advertise::media_addresses(local, target_addr.ip(), advertised);
    let media_address = media_addresses.advertised;
    let from = options
        .from
        .as_deref()
        .map_or_else(|| format!("<sip:sipx@{media_address}>"), str::to_owned);
    let credentials = match password {
        Some(password) => {
            let username = Address::parse(from.as_bytes(), "From")
                .ok()
                .and_then(|address| address.uri.decoded_user())
                .map(|user| String::from_utf8_lossy(&user).into_owned());
            let Some(username) = username else {
                return fail(
                    format,
                    Exit::Usage,
                    "--password requires --from to contain a SIP username",
                );
            };
            Some(Credentials::new(username, password))
        }
        None => None,
    };

    let mut config = TransportConfig::new(local);
    config.sent_by = media_address.to_string();
    crate::apply_capture(&options.capture, &mut config);
    if let Err(message) = transport.configure_client(&options.signalling, &mut config) {
        return fail(format, Exit::Usage, &message);
    }
    let (handle, _incoming) = match bind(config).await {
        Ok(bound) => bound,
        Err(error) => return fail(format, Exit::Failed, &format!("bind: {error}")),
    };

    // Armed here, not at the end: the run that most needs the numbers is the one that fails, and
    // every `return fail(…)` below now takes the counters file with it.
    let export = crate::counters::Export::arm(&options.capture, &handle);

    // The bound is handed to the library rather than wrapped around it. Dropping the call
    // future partway through would leave the far end believing it is in a call, and only code
    // inside the exchange can send the CANCEL that stops it ringing.
    let attempt = Duration::from_secs(options.timeout);

    let mut call_options = sipx_call::DialOptions::new(from, media_address)
        .with_media_bind_address(media_addresses.bind)
        .with_media_policy(media.policy());
    for header in headers {
        call_options = call_options.with_header(header);
    }
    if !attempt.is_zero() {
        call_options = call_options.with_timeout(attempt);
    }
    if let Some(credentials) = credentials {
        call_options = call_options.with_credentials(credentials);
    }

    // What `-v` is *for*, and what it used to be worth nothing on: a call's own progress. The
    // workspace's only INFO records were a registration refresh and a transcoding bridge, so
    // `sipx dial -v` said nothing whatsoever through a call that worked, and an operator reading the
    // help text got silence from the level it documents (`X-57`). These three records are the
    // lifecycle a shell already sees on stdout — placed, answered, ended — on the stream that is
    // allowed to narrate it.
    tracing::info!(peer = uri, transport = ?negotiated_transport, "calling");

    let duration = Duration::from_secs(options.duration);
    let early_requested = options.early_media;
    let started = std::time::Instant::now();
    let (mut call, early_recorded, early_media) = if early_requested {
        let mut dialing = match sipx_call::dial_early(&handle, target, &to, &call_options).await {
            Ok(dialing) => dialing,
            Err(error) => return report_failure(format, &error),
        };
        let early_media = match dialing.wait_for_early_media().await {
            Ok(available) => available,
            Err(error) => return report_failure(format, &error),
        };
        let early_recorded = if early_media {
            let Some(session) = dialing.media() else {
                return fail(
                    format,
                    Exit::Failed,
                    "early media was reported without a running media session",
                );
            };
            crate::record_media(session, duration, crate::RECORD_IDLE).await
        } else {
            Vec::new()
        };
        let call = match dialing.answered().await {
            Ok(call) => call,
            Err(error) => return report_failure(format, &error),
        };
        (call, early_recorded, early_media)
    } else {
        let call = match sipx_call::dial(&handle, target, &to, &call_options).await {
            Ok(call) => call,
            Err(error) => return report_failure(format, &error),
        };
        (call, Vec::new(), false)
    };
    tracing::info!(peer = uri, setup = ?started.elapsed(), "answered");

    let exchanged = match exchange(
        &mut call,
        clip.as_ref(),
        options.dtmf.as_deref(),
        duration,
        &mut devices,
    )
    .await
    {
        Ok(exchanged) => exchanged,
        Err(message) => {
            let _ = call.hang_up().await;
            return fail(format, Exit::Failed, &message);
        }
    };

    let early_samples = early_recorded.len();
    let total_samples = early_samples.saturating_add(exchanged.samples_received);
    let report = media.requested_report(
        Report::new()
            .text("status", "answered")
            .text("peer", uri)
            .text("media_advertised", media_address.to_string())
            .text("media_bound", call.media().local_addr().to_string())
            .number(
                "duration_ms",
                i64::try_from(started.elapsed().as_millis()).unwrap_or(0),
            )
            .number(
                "samples_recorded",
                i64::try_from(total_samples).unwrap_or(i64::MAX),
            )
            .boolean("heard_audio", total_samples != 0),
    );
    let report = media.negotiated_report(report, &call, "browser-offerer");
    let mut report = devices.report(transport.report(report, negotiated_transport));
    if early_requested {
        report = report.boolean("early_media", early_media).number(
            "early_samples_recorded",
            i64::try_from(early_samples).unwrap_or(i64::MAX),
        );
    }

    if options.stats {
        report = with_quality(report, &call.media().quality().await);
    }

    if let Some(path) = audio.wav_output() {
        let mut recorded = early_recorded;
        recorded.extend_from_slice(&exchanged.recorded);
        match write_clip(path, &recorded, call.media().clock_rate()) {
            Ok(()) => report = report.text("recording", path),
            Err(message) => return fail(format, Exit::Failed, &message),
        }
    }

    let _ = call.hang_up().await;
    tracing::info!(
        peer = uri,
        elapsed = ?started.elapsed(),
        samples_recorded = exchanged.samples_received,
        "hung up"
    );
    report = match export.into_report(report) {
        Ok(report) => report,
        Err(message) => return fail(format, Exit::Failed, &message),
    };
    report.emit(format);
    Exit::Success
}

/// Play, send digits and record for the duration of the call.
async fn exchange(
    call: &mut Call,
    clip: Option<&Wav>,
    dtmf: Option<&str>,
    duration: Duration,
    devices: &mut crate::device::Driver,
) -> Result<Exchange, String> {
    let media = call.media();

    let playing = async {
        if let Some(clip) = clip {
            let pcm = pcm_clip(clip)?;
            call.play_pcm(&pcm)
                .await
                .map_err(|error| error.to_string())?;
        }
        if let Some(digits) = dtmf {
            // After the audio, so a menu hears the prompt before the keypress.
            call.send_digits(digits, Duration::from_millis(100)).await;
        }
        Ok::<(), String>(())
    };

    // Recording stops when the far end goes quiet, or when the call's time is up — whichever comes
    // first, so a peer that never stops talking cannot hold the command open. Waiting for it to
    // *start* is a separate bound, and conflating the two is what `X-40` was: one 500 ms window
    // answered both questions, so audio that began later than it was not recorded at all. The cap
    // lives inside `crate::record` now, which is also what stops a recording the cap cut short from
    // being discarded by an `unwrap_or_default` out here.
    let output_device = devices.has_output();
    let recording = async {
        if output_device {
            Vec::new()
        } else {
            crate::record(call, duration, crate::RECORD_IDLE).await
        }
    };
    let device_audio = devices.run(media, duration);

    let (played, recorded, device_samples) = tokio::join!(
        tokio::time::timeout(duration, playing),
        recording,
        device_audio
    );
    if let Ok(Err(error)) = played {
        return Err(error);
    }
    let device_samples = device_samples?;
    let samples_received = if output_device {
        usize::try_from(device_samples).unwrap_or(usize::MAX)
    } else {
        recorded.len()
    };
    Ok(Exchange {
        recorded,
        samples_received,
    })
}

struct Exchange {
    recorded: Vec<i16>,
    samples_received: usize,
}

fn report_failure(format: Format, error: &sipx_call::Error) -> Exit {
    let exit = match error {
        sipx_call::Error::Rejected { status, .. } => Exit::for_status(*status),
        sipx_call::Error::NoResponse | sipx_call::Error::Cancelled(_) => Exit::Timeout,
        _ => Exit::Failed,
    };
    fail(format, exit, &error.to_string())
}

/// Read and structurally validate a clip before signalling starts.
pub(crate) fn read_clip(path: &str) -> Result<Wav, String> {
    let file = std::fs::File::open(path).map_err(|error| format!("{path}: {error}"))?;
    let clip = read_wav(file).map_err(|error| format!("{path}: {error}"))?;
    sipx_audio::PcmFormat::new(clip.sample_rate, sipx_audio::PcmEncoding::Signed16)
        .map_err(|error| format!("{path}: {error}"))?;
    Ok(clip)
}

/// Attach a WAV's explicit signed-16 representation to its declared sample rate.
pub(crate) fn pcm_clip(clip: &Wav) -> Result<sipx_audio::Pcm, String> {
    let format = sipx_audio::PcmFormat::new(clip.sample_rate, sipx_audio::PcmEncoding::Signed16)
        .map_err(|error| error.to_string())?;
    sipx_audio::Pcm::new(
        format,
        sipx_audio::PcmSamples::Signed16(clip.samples.clone()),
    )
    .map_err(|error| error.to_string())
}

pub(crate) fn write_clip(path: &str, samples: &[i16], sample_rate: u32) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|error| format!("{path}: {error}"))?;
    write_wav(
        file,
        &Wav {
            sample_rate,
            samples: samples.to_vec(),
        },
    )
    .map_err(|error| format!("{path}: {error}"))
}

/// The address and port a URI names, if it names one directly.
pub(crate) fn target_of(
    uri: &Uri,
    transport: TransportKind,
) -> Option<(std::net::SocketAddr, String)> {
    let Host::Ip(ip) = uri.host()? else {
        return None;
    };
    let port = uri.port().unwrap_or_else(|| transport.default_port());
    Some((std::net::SocketAddr::new(*ip, port), ip.to_string()))
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
    use clap::Parser as _;

    use crate::cli::{Cli, Command};
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

    fn target(input: &str, transport: TransportKind) -> Option<std::net::SocketAddr> {
        let uri = Uri::parse(bytes::Bytes::from(input.to_owned())).expect("a URI");
        target_of(&uri, transport).map(|(addr, _)| addr)
    }

    #[test]
    fn a_uri_naming_an_address_and_port_is_the_target() {
        assert_eq!(
            target("sip:bob@192.0.2.1:5080", TransportKind::Udp).map(|a| a.to_string()),
            Some("192.0.2.1:5080".to_owned())
        );
    }

    #[test]
    fn a_uri_without_a_port_gets_the_default() {
        assert_eq!(
            target("sip:bob@192.0.2.1", TransportKind::Udp).map(|a| a.to_string()),
            Some("192.0.2.1:5060".to_owned())
        );
    }

    #[test]
    fn a_uri_with_no_user_still_yields_its_host() {
        assert_eq!(
            target("sip:192.0.2.1:5060", TransportKind::Udp).map(|a| a.to_string()),
            Some("192.0.2.1:5060".to_owned())
        );
    }

    #[test]
    fn uri_parameters_are_not_part_of_the_host() {
        assert_eq!(
            target("sip:bob@192.0.2.1:5060;transport=tcp", TransportKind::Tcp)
                .map(|a| a.to_string()),
            Some("192.0.2.1:5060".to_owned())
        );
    }

    #[test]
    fn a_name_is_not_a_target_this_command_can_use() {
        assert!(target("sip:bob@example.com", TransportKind::Udp).is_none());
    }

    /// The behavioural half of `S-27`, and the one that actually matters: the *command* refuses,
    /// not just a helper. This needs no network and no peer, which is the point — the check happens
    /// before a transport is chosen or an address parsed, so there is no window in which a cleartext
    /// datagram could leave.
    /// The behavioural half of `S-27`, and the one that matters: the *command* refuses, not just a
    /// helper. It needs no network and no peer, which is the point — the refusal happens before a
    /// transport is chosen, so there is no window in which a cleartext datagram could leave.
    ///
    #[tokio::test]
    async fn the_dial_command_refuses_a_sips_uri_before_touching_the_network() {
        let parsed =
            Cli::try_parse_from(["sipx", "dial", "sips:bob@192.0.2.1"]).expect("valid syntax");
        let Some(Command::Dial(options)) = parsed.command else {
            panic!("dial command expected");
        };
        let exit = Box::pin(run(options, Format::Text)).await;
        assert_eq!(
            exit.code(),
            Exit::Usage.code(),
            "dialling a sips: URI must be refused, not connected in the clear"
        );
    }

    /// M-43: a WAV rate different from the codec clock is accepted for explicit resampling.
    #[test]
    fn a_clip_at_a_different_sample_rate_is_accepted() {
        let dir = std::env::temp_dir().join(format!("sipx-dial-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let path = dir.join("wide.wav");

        let wide = Wav {
            sample_rate: 44_100,
            samples: vec![0; 100],
        };
        write_wav(std::fs::File::create(&path).expect("creates"), &wide).expect("writes");

        let clip = read_clip(path.to_str().expect("a path")).expect("structurally valid");
        assert_eq!(clip.sample_rate, 44_100);

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
