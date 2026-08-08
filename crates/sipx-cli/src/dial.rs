//! `sipx dial`.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use sipx_audio::{Wav, read_wav};
use sipx_call::{Call, Credentials, EndCause, Served};
use sipx_sip::{Address, Uri};
use sipx_transport::{Config as TransportConfig, bind};

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
    let mut transport = match crate::signalling::Selection::from_options(
        &options.signalling,
        to.scheme().is_secure(),
    ) {
        Ok(transport) => transport,
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

    // Local file failures are knowable without a peer. Read input and reserve output before even
    // destination resolution, which may itself perform network I/O.
    let clip = match audio.wav_input().map(read_clip) {
        Some(Ok(clip)) => Some(clip),
        Some(Err(message)) => return fail(format, Exit::Usage, &message),
        None => None,
    };
    let recording = match audio.reserve_wav_output() {
        Ok(recording) => recording,
        Err(message) => return fail(format, Exit::Usage, &message),
    };
    let mut devices = match audio.open() {
        Ok(devices) => devices,
        Err(message) => return fail(format, Exit::Failed, &message),
    };

    let password = options.password.clone();

    // The invitation budget is read here, before the first thing that can wait, because it is the
    // ceiling over resolution too: `dial --timeout 5` against a name nothing answers for must give
    // up in five seconds, not spend `T-38`'s own eight looking it up first. Zero states no
    // deadline and leaves those bounds to the resolver, exactly as it leaves expiry to the
    // transaction layer.
    let attempt = Duration::from_secs(options.timeout);
    let resolver = crate::destination::Resolver::within((!attempt.is_zero()).then_some(attempt));
    let candidates = match resolver
        .resolve(&to, None, transport, &options.signalling)
        .await
    {
        Ok(candidates) => candidates,
        Err(error) => return fail(format, error.exit(), &error.to_string()),
    };
    let target = match crate::destination::first(&candidates) {
        Ok(target) => target.clone(),
        Err(error) => return fail(format, error.exit(), &error.to_string()),
    };
    let target_addr = target.addr;
    transport = transport.negotiated(target.transport);
    let media = match crate::media::Selection::from_options(
        &options.media,
        transport.kind(),
        options.early_media,
    ) {
        Ok(media) => media,
        Err(message) => return fail(format, Exit::Usage, &message),
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
    let mut progress = crate::progress::Call::new(crate::progress::CallRole::Dial, uri);
    let (handle, mut incoming) = match bind(config).await {
        Ok(bound) => bound,
        Err(error) => return fail(format, Exit::Failed, &format!("bind: {error}")),
    };

    // Armed here, not at the end: the run that most needs the numbers is the one that fails, and
    // every `return fail(…)` below now takes the counters file with it.
    let export = crate::counters::Export::arm(&options.capture, &handle);
    let process_stop = crate::stop::Stop::new();
    let interrupted = process_stop.wait();
    tokio::pin!(interrupted);

    // The bound is handed to the library rather than wrapped around it. Dropping the call
    // future partway through would leave the far end believing it is in a call, and only code
    // inside the exchange can send the CANCEL that stops it ringing.
    let cancellation = Duration::from_secs(options.cancel_timeout);

    let mut call_options = sipx_call::DialOptions::new(from, media_address)
        .with_media_bind_address(media_addresses.bind)
        .with_media_policy(media.policy())
        .with_cancellation_timeout(cancellation);
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
    progress.placed(target.transport);

    let duration = Duration::from_secs(options.duration);
    let early_requested = options.early_media;
    let (mut call, selected_target, early_recorded, early_media) = if early_requested {
        let (mut dialing, selected) = match dial_early_candidates(
            &handle,
            &candidates,
            &to,
            &call_options,
            interrupted.as_mut(),
        )
        .await
        {
            Ok(dialing) => dialing,
            Err(sipx_call::Error::Cancelled(cancellation)) if !cancellation.timed_out => {
                return report_pending_interrupt(
                    format,
                    export,
                    &handle,
                    &process_stop,
                    cancellation,
                    &mut progress,
                )
                .await;
            }
            Err(error) => {
                return report_failure(format, export, &handle, &error, &mut progress).await;
            }
        };
        let early_media = match tokio::select! {
            biased;
            () = interrupted.as_mut() => {
                let cancellation = dialing.cancel_observed().await;
                return report_pending_interrupt(
                    format,
                    export,
                    &handle,
                    &process_stop,
                    cancellation,
                    &mut progress,
                )
                .await;
            }
            available = dialing.wait_for_early_media() => available,
        } {
            Ok(available) => available,
            Err(error) => {
                return report_failure(format, export, &handle, &error, &mut progress).await;
            }
        };
        let early_recorded = if early_media {
            let Some(session) = dialing.media() else {
                drop(dialing);
                handle.shutdown().await;
                return fail(
                    format,
                    Exit::Failed,
                    "early media was reported without a running media session",
                );
            };
            tokio::select! {
                biased;
                () = interrupted.as_mut() => {
                    let cancellation = dialing.cancel_observed().await;
                    return report_pending_interrupt(
                        format,
                        export,
                        &handle,
                        &process_stop,
                        cancellation,
                        &mut progress,
                    )
                    .await;
                }
                recorded = crate::record_media(session, duration, crate::RECORD_IDLE) => recorded,
            }
        } else {
            Vec::new()
        };
        let call = match dialing.answered_until(interrupted.as_mut()).await {
            Ok(call) => call,
            Err(sipx_call::Error::Cancelled(cancellation)) if !cancellation.timed_out => {
                return report_pending_interrupt(
                    format,
                    export,
                    &handle,
                    &process_stop,
                    cancellation,
                    &mut progress,
                )
                .await;
            }
            Err(error) => {
                return report_failure(format, export, &handle, &error, &mut progress).await;
            }
        };
        (call, selected, early_recorded, early_media)
    } else {
        let (call, selected) = match dial_candidates(
            &handle,
            &candidates,
            &to,
            &call_options,
            interrupted.as_mut(),
        )
        .await
        {
            Ok(call) => call,
            Err(sipx_call::Error::Cancelled(cancellation)) if !cancellation.timed_out => {
                return report_pending_interrupt(
                    format,
                    export,
                    &handle,
                    &process_stop,
                    cancellation,
                    &mut progress,
                )
                .await;
            }
            Err(error) => {
                return report_failure(format, export, &handle, &error, &mut progress).await;
            }
        };
        (call, selected, Vec::new(), false)
    };
    let negotiated_transport = selected_target.transport;
    progress.answered();

    let served = sipx_call::serve_until(
        &mut call,
        &mut incoming,
        |media, cancelled| {
            exchange(
                media,
                clip.as_ref(),
                options.dtmf.as_deref(),
                duration,
                &mut devices,
                cancelled,
            )
        },
        interrupted.as_mut(),
    )
    .await;
    devices.stop();
    let served = match served {
        Ok(served) => served,
        Err(error) => {
            drop(call);
            handle.shutdown().await;
            return fail(format, Exit::Failed, &error.to_string());
        }
    };
    let (exchanged, end, bye) = match served {
        Served::Remote {
            cause: EndCause::RemoteBye,
            output,
        } => (output, crate::progress::CallEnd::Remote, None),
        Served::Remote { cause, output } => {
            let _ = output;
            drop(call);
            handle.shutdown().await;
            return fail(
                format,
                Exit::Failed,
                &format!("call ended unexpectedly: {cause:?}"),
            );
        }
        Served::Local { output, bye } => (output, crate::progress::CallEnd::Duration, Some(bye)),
        Served::Interrupted { output, bye } => {
            (output, crate::progress::CallEnd::Interrupted, Some(bye))
        }
        _ => {
            drop(call);
            handle.shutdown().await;
            return fail(format, Exit::Failed, "unknown confirmed-call outcome");
        }
    };
    let exchanged = match exchanged {
        Ok(exchanged) => exchanged,
        Err(message) => {
            drop(call);
            handle.shutdown().await;
            return fail(format, Exit::Failed, &message);
        }
    };
    let bye_status = match bye.transpose() {
        Ok(status) => status,
        Err(sipx_call::Error::SignallingTeardownTimeout(_)) => None,
        Err(error) => {
            drop(call);
            handle.shutdown().await;
            return fail(format, Exit::Failed, &error.to_string());
        }
    };

    let early_samples = early_recorded.len();
    let total_samples = early_samples.saturating_add(exchanged.samples_received);
    let status = end.status();
    let ended_by = end.ended_by();
    let report = media.requested_report(
        Report::new()
            .text("status", status)
            .text("ended_by", ended_by)
            .text("peer", uri)
            .text("media_advertised", media_address.to_string())
            .text("media_bound", call.media().local_addr().to_string())
            .number(
                "duration_ms",
                i64::try_from(progress.elapsed().as_millis()).unwrap_or(0),
            )
            .number(
                "samples_recorded",
                i64::try_from(total_samples).unwrap_or(i64::MAX),
            )
            .boolean("heard_audio", total_samples != 0),
    );
    let report = media.negotiated_report(report, &call, "browser-offerer");
    let mut report = devices.report(transport.report(report, negotiated_transport));
    if status == "interrupted"
        && let Some(signal) = process_stop.signal()
    {
        report = report.text("stop_signal", signal);
    }
    if let Some(status) = bye_status {
        report = report.number("bye_status", i64::from(status));
    }
    if early_requested {
        report = report.boolean("early_media", early_media).number(
            "early_samples_recorded",
            i64::try_from(early_samples).unwrap_or(i64::MAX),
        );
    }

    if options.stats {
        report = with_quality(report, &call.media().quality().await);
    }

    if let Some(recording) = recording {
        let mut recorded = early_recorded;
        recorded.extend_from_slice(&exchanged.recorded);
        // The WAV header carries the audio rate, which for G.722 is 16 kHz — twice the RTP
        // clock (RFC 3551 §4.5.2).
        match recording.finish(&recorded, call.media().audio_rate()) {
            Ok(path) => report = report.text("recording", path),
            Err(message) => {
                drop(call);
                handle.shutdown().await;
                return fail(format, Exit::Failed, &message);
            }
        }
    }

    drop(call);
    handle.shutdown().await;
    if let Some(message) = process_stop.failure() {
        return fail(format, Exit::Failed, &message);
    }
    report = match export.into_report(report) {
        Ok(report) => report,
        Err(message) => return fail(format, Exit::Failed, &message),
    };
    progress.finish(end);
    report.emit(format);
    Exit::Success
}

/// Try only concrete transport failures on the next RFC 3263 candidate. SIP refusals and response
/// deadlines belong to the transaction that was sent and are never rewritten as routing retries.
async fn dial_candidates(
    handle: &sipx_transport::Handle,
    candidates: &[sipx_transport::Target],
    to: &Uri,
    options: &sipx_call::DialOptions,
    mut interrupted: Pin<&mut (dyn Future<Output = ()> + Send)>,
) -> Result<(Call, sipx_transport::Target), sipx_call::Error> {
    let mut last_transport = None;
    for target in candidates.iter().take(crate::destination::MAX_ATTEMPTS) {
        match sipx_call::dial_until(handle, target.clone(), to, options, interrupted.as_mut()).await
        {
            Ok(call) => return Ok((call, target.clone())),
            Err(error @ sipx_call::Error::Transport(_)) => last_transport = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(last_transport.unwrap_or(sipx_call::Error::NoResponse))
}

async fn dial_early_candidates(
    handle: &sipx_transport::Handle,
    candidates: &[sipx_transport::Target],
    to: &Uri,
    options: &sipx_call::DialOptions,
    mut interrupted: Pin<&mut (dyn Future<Output = ()> + Send)>,
) -> Result<(sipx_call::Dialing, sipx_transport::Target), sipx_call::Error> {
    let mut last_transport = None;
    for target in candidates.iter().take(crate::destination::MAX_ATTEMPTS) {
        match sipx_call::dial_early_until(handle, target.clone(), to, options, interrupted.as_mut())
            .await
        {
            Ok(dialing) => return Ok((dialing, target.clone())),
            Err(error @ sipx_call::Error::Transport(_)) => last_transport = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(last_transport.unwrap_or(sipx_call::Error::NoResponse))
}

/// Play, send digits and record for the duration of the call.
async fn exchange(
    media: std::sync::Arc<sipx_media::MediaSession>,
    clip: Option<&Wav>,
    dtmf: Option<&str>,
    duration: Duration,
    devices: &mut crate::device::Driver,
    cancelled: tokio_util::sync::CancellationToken,
) -> Result<Exchange, String> {
    let playing = async {
        if let Some(clip) = clip {
            let pcm = pcm_clip(clip)?;
            media
                .play_pcm(&pcm)
                .await
                .map_err(|error| error.to_string())?;
        }
        if let Some(digits) = dtmf {
            // After the audio, so a menu hears the prompt before the keypress.
            for character in digits.chars() {
                if let Some(digit) = sipx_rtp::Digit::from_char(character) {
                    let _ = media.send_digit(digit, Duration::from_millis(100)).await;
                }
            }
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
            crate::record_media(&media, duration, crate::RECORD_IDLE).await
        }
    };
    let device_audio = devices.run(&media, duration, &cancelled);

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

/// Report a deliberate stop which the call layer has already drained as CANCEL or late-2xx BYE.
async fn report_pending_interrupt(
    format: Format,
    export: crate::counters::Export,
    handle: &sipx_transport::Handle,
    process_stop: &crate::stop::Stop,
    cancellation: sipx_call::InvitationCancellation,
    progress: &mut crate::progress::Call,
) -> Exit {
    handle.shutdown().await;
    if let Some(message) = process_stop.failure() {
        return fail(format, Exit::Failed, &message);
    }
    let mut report = with_cancellation(
        Report::new()
            .text("status", "interrupted")
            .text("ended_by", "interrupt"),
        &cancellation,
    );
    if let Some(signal) = process_stop.signal() {
        report = report.text("stop_signal", signal);
    }
    match export.into_report(report) {
        Ok(report) => {
            progress.finish(crate::progress::CallEnd::Interrupted);
            report.emit(format);
            Exit::Success
        }
        Err(message) => {
            progress.finish(crate::progress::CallEnd::Failed);
            fail(format, Exit::Failed, &message)
        }
    }
}

async fn report_failure(
    format: Format,
    export: crate::counters::Export,
    handle: &sipx_transport::Handle,
    error: &sipx_call::Error,
    progress: &mut crate::progress::Call,
) -> Exit {
    let (exit, end) = match error {
        sipx_call::Error::Rejected { status, .. } => {
            let exit = Exit::for_status(*status);
            (exit, crate::progress::CallEnd::Refused(exit.as_str()))
        }
        sipx_call::Error::NoResponse | sipx_call::Error::Cancelled(_) => {
            (Exit::Timeout, crate::progress::CallEnd::Timeout)
        }
        _ => (Exit::Failed, crate::progress::CallEnd::Failed),
    };
    handle.shutdown().await;
    let report = Report::new()
        .text("status", exit.as_str())
        .text("error", error.to_string());
    let report = match error {
        sipx_call::Error::Cancelled(cancellation) => with_cancellation(report, cancellation),
        _ => report,
    };
    match export.into_report(report) {
        Ok(report) => {
            progress.finish(end);
            eprintln!("{}", report.render(format));
            exit
        }
        Err(message) => {
            progress.finish(crate::progress::CallEnd::Failed);
            fail(format, Exit::Failed, &message)
        }
    }
}

fn with_cancellation(
    mut report: Report,
    cancellation: &sipx_call::InvitationCancellation,
) -> Report {
    if let Some(limit) = cancellation.invitation_limit {
        report = report.millis("invitation_limit_ms", limit);
    }
    report
        .millis("invitation_elapsed_ms", cancellation.invitation_elapsed)
        .millis("cancel_limit_ms", cancellation.cleanup.limit)
        .millis("cancel_elapsed_ms", cancellation.cleanup.elapsed)
        .boolean("cancel_sent", cancellation.cleanup.cancel_sent())
        .boolean(
            "cancel_final_observed",
            cancellation.cleanup.final_response_observed(),
        )
        .boolean("cancel_cleanup_completed", cancellation.cleanup.completed())
        .boolean("cancel_cleanup_exhausted", cancellation.cleanup.exhausted())
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
    use sipx_audio::write_wav;

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
