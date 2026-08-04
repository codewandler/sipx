//! `sipx answer`.

use std::net::IpAddr;
use std::time::Duration;

use sipx_sip::{HeaderName, StatusCode};
use sipx_transport::{Config as TransportConfig, bind};

use crate::output::{Exit, Format, Report, fail};

pub(crate) const HELP: &str = "\
sipx answer — wait for a call and answer it

USAGE:
    sipx answer [OPTIONS]

OPTIONS:
    --play <FILE>     Play mono 16-bit WAV at the negotiated codec clock
    --record <FILE>   Record the caller to WAV at the negotiated codec clock
    --duration <S>    Hang up after this many seconds (default 30)
    --wait <S>        Give up if no call arrives within this many seconds (default 60)
    --local <ADDR>    Local address to bind (default 0.0.0.0:5060)
    --transport <T>   Signalling: udp, tcp, tls, ws or wss (no flag keeps UDP/TCP)
    --tcp             Legacy alias for --transport tcp
    --tls-cert <FILE> Server certificate chain for TLS/WSS (with --tls-key)
    --tls-key <FILE>  Server private key for TLS/WSS (with --tls-cert)
    --codec <C>       Ordered codec preference; repeat pcmu, pcma or opus (default pcmu, pcma)
    --media-security <M>  auto, plain, sdes or dtls-srtp (default auto)
    --ice <P>         disabled, host or stun (default disabled)
    --stun-server <ADDR>  STUN server for --ice stun, as host:port
    --audio-input <E>  Local source: wav:<path>, device:<id> or null
    --audio-output <E> Local sink: wav:<path>, device:<id> or null
    --header <H>      Add an application-owned final-response field; repeat 'Name: value'
    --reject          Answer 603 Decline instead
    --busy            Answer 486 Busy Here instead
    --once            Exit after one call (default; kept for clarity in scripts)
    --capture <FILE>  Record signalling to this pcapng file. Credentials are redacted;
                      TLS is recorded decrypted. Still identifies who called whom
    --counters <FILE> Write this run's signalling counters to this file, as JSON.
                      Implied by --capture, as <capture>.counters.json
    --json            Report as JSON
";

#[allow(
    clippy::too_many_lines,
    reason = "the command lifecycle is kept in execution order so validation-before-I/O remains auditable"
)]
pub(crate) async fn run(raw: &[String], format: Format) -> Exit {
    // Refused before the socket is bound, so a dropped flag cannot become an answerer that
    // reports `listening` and then records nothing (`S-30`).
    let args = match crate::arguments(raw, HELP, format) {
        Ok(args) => args,
        Err(exit) => return exit,
    };

    let transport = match crate::signalling::Selection::from_args(&args, false) {
        Ok(transport) => transport,
        Err(message) => return fail(format, Exit::Usage, &message),
    };
    let media = match crate::media::Selection::from_args(&args, transport.kind()) {
        Ok(media) => media,
        Err(message) => return fail(format, Exit::Usage, &message),
    };
    let audio = match crate::device::Selection::from_args(&args) {
        Ok(audio) => audio,
        Err(message) => return fail(format, Exit::Usage, &message),
    };
    let headers = match crate::header::from_args(&args) {
        Ok(headers) => headers,
        Err(message) => return fail(format, Exit::Usage, &message),
    };

    let clip = match audio.wav_input().map(crate::dial::read_clip) {
        Some(Ok(clip)) => Some(clip),
        Some(Err(message)) => return fail(format, Exit::Usage, &message),
        None => None,
    };
    let mut devices = match audio.open() {
        Ok(devices) => devices,
        Err(message) => return fail(format, Exit::Failed, &message),
    };

    let local: std::net::SocketAddr = match args.value("local").unwrap_or("0.0.0.0:5060").parse() {
        Ok(local) => local,
        Err(_) => return fail(format, Exit::Usage, "--local must be host:port"),
    };

    let mut config = TransportConfig::new(local);
    if local.ip().is_unspecified() {
        "127.0.0.1".clone_into(&mut config.sent_by);
    }
    crate::apply_capture(&args, &mut config);
    if let Err(message) = transport.configure_listener(&args, &mut config) {
        return fail(format, Exit::Usage, &message);
    }
    let (handle, mut incoming) = match bind(config).await {
        Ok(bound) => bound,
        Err(error) => return fail(format, Exit::Failed, &format!("bind: {error}")),
    };

    // Armed here, not at the end: the run that most needs the numbers is the one that fails, and
    // every `return fail(…)` below now takes the counters file with it.
    let export = crate::counters::Export::arm(&args, &handle);

    // Announce the port before waiting, so a script that started this in the background knows
    // where to call without guessing or racing.
    let Some(listening) = transport.listener_addr(&handle) else {
        return fail(
            format,
            Exit::Failed,
            "the selected signalling listener did not bind",
        );
    };
    transport
        .requested_report(
            media.requested_report(
                Report::new()
                    .text("status", "listening")
                    .text("address", listening.to_string()),
            ),
        )
        .emit(format);

    let wait = Duration::from_secs(args.number("wait").unwrap_or(60));
    // The call's progress at INFO, which is what `-v` documents and what it produced nothing of
    // before `X-57`: the two INFO records in the workspace are a registration refresh and a
    // transcoding bridge, and a call goes near neither.
    tracing::info!(address = %listening, within = ?wait, "waiting for a call");
    let deadline = tokio::time::Instant::now() + wait;
    let request = loop {
        let Ok(Some(request)) = tokio::time::timeout_at(deadline, incoming.recv()).await else {
            return fail(
                format,
                Exit::Timeout,
                "no call arrived on the selected transport",
            );
        };
        if transport.accepts(request.transport) {
            break request;
        }
    };

    let caller = request
        .request
        .headers
        .value(&HeaderName::From)
        .map(|value| String::from_utf8_lossy(&value).into_owned())
        .unwrap_or_default();

    if args.flag("reject") || args.flag("busy") {
        return refuse(
            &handle,
            &request,
            &caller,
            args.flag("busy"),
            transport,
            format,
            &headers,
        )
        .await;
    }

    let media_address: IpAddr = if local.ip().is_unspecified() {
        // Answer on the address the caller reached us at, which is the one they can hear.
        request.source.ip()
    } else {
        local.ip()
    };

    let started = std::time::Instant::now();
    let mut call = match sipx_call::answer_with_policy_and_headers(
        &handle,
        &request,
        media_address,
        media.policy(),
        &headers,
    )
    .await
    {
        Ok(call) => call,
        Err(error) => return fail(format, Exit::Failed, &error.to_string()),
    };

    let duration = Duration::from_secs(args.number("duration").unwrap_or(30));
    let session = call.media();
    if let (Some(path), Some(clip)) = (audio.wav_input(), clip.as_ref())
        && let Err(message) = crate::dial::validate_clip(path, clip, session.codec().clock_rate())
    {
        let _ = call.hang_up().await;
        return fail(format, Exit::Usage, &message);
    }

    // The recording bounds itself, and keeps whatever arrived (`X-40`). It used to be
    // `timeout(duration, media.record_until_idle(500ms))`, which spent one 500 ms window on both
    // "has the stream started" and "has it ended" — so a first frame delayed past it recorded
    // nothing at all — and then `unwrap_or_default` threw away any partial recording the cap cut
    // short. `crate::record` separates the two bounds and returns what it got.
    //
    // The digits had both defects on one line too, and `M-34` split them the same way: the wait
    // for the first keypress is the call's own duration — a caller cannot be quicker than the
    // call — while `DIGIT_GAP` is left with the only question it can answer, whether the caller
    // has stopped dialling. `collect_digits` enforces the cap itself, so there is no timed-out
    // future left to `unwrap_or_default` the collected digits away.
    let output_device = devices.has_output();
    let ((), recorded, digits, device_samples) = tokio::join!(
        async {
            if let Some(clip) = &clip {
                let _ = tokio::time::timeout(
                    duration,
                    session.play(&clip.samples, session.samples_per_packet()),
                )
                .await;
            }
        },
        async {
            if output_device {
                Vec::new()
            } else {
                crate::record(&call, duration, crate::RECORD_IDLE).await
            }
        },
        session.collect_digits(duration, crate::DIGIT_GAP),
        devices.run(session, duration),
    );
    let device_samples = match device_samples {
        Ok(samples) => samples,
        Err(message) => {
            let _ = call.hang_up().await;
            return fail(format, Exit::Failed, &message);
        }
    };
    let samples_received = if output_device {
        usize::try_from(device_samples).unwrap_or(usize::MAX)
    } else {
        recorded.len()
    };

    let report = media.requested_report(
        Report::new()
            .text("status", "answered")
            .text("caller", caller)
            .number(
                "duration_ms",
                i64::try_from(started.elapsed().as_millis()).unwrap_or(0),
            )
            .number(
                "samples_recorded",
                i64::try_from(samples_received).unwrap_or(i64::MAX),
            )
            .boolean("heard_audio", samples_received != 0),
    );
    let report = media.negotiated_report(report, &call);
    let mut report = devices.report(transport.report(report, request.transport));

    if !digits.is_empty() {
        report = report.text("dtmf", digits);
    }

    if let Some(path) = audio.wav_output() {
        match crate::dial::write_clip(path, &recorded, session.codec().clock_rate()) {
            Ok(()) => report = report.text("recording", path),
            Err(message) => return fail(format, Exit::Failed, &message),
        }
    }

    let _ = call.hang_up().await;
    report = match export.into_report(report) {
        Ok(report) => report,
        Err(message) => return fail(format, Exit::Failed, &message),
    };
    report.emit(format);
    Exit::Success
}

/// Answer with a refusal rather than taking the call.
async fn refuse(
    handle: &sipx_transport::Handle,
    request: &sipx_transport::Incoming,
    caller: &str,
    busy: bool,
    transport: crate::signalling::Selection,
    format: Format,
    headers: &[sipx_sip::Header],
) -> Exit {
    let (code, reason) = if busy {
        (486, "Busy Here")
    } else {
        (603, "Decline")
    };
    let Some(status) = StatusCode::new(code) else {
        return fail(format, Exit::Failed, "bad status");
    };
    let builder =
        match sipx_sip::build::ResponseBuilder::to_request(&request.request, status, reason) {
            Ok(builder) => builder,
            Err(error) => return fail(format, Exit::Failed, &error.to_string()),
        };
    // RFC 3261 §8.2.6.2: every response but 100 carries a To tag — it is what lets a caller
    // behind a forking proxy tell this branch's refusal from another's.
    let mut builder = match request.request.headers.value(&HeaderName::To) {
        Some(to) => {
            let to = tagged(&String::from_utf8_lossy(&to), &fresh_tag());
            match builder.set_header(&HeaderName::To, bytes::Bytes::from(to)) {
                Ok(builder) => builder,
                Err(error) => return fail(format, Exit::Failed, &error.to_string()),
            }
        }
        None => builder,
    };
    for header in headers {
        builder = match builder.header(
            header.name().clone(),
            bytes::Bytes::copy_from_slice(header.raw_value()),
        ) {
            Ok(builder) => builder,
            Err(error) => return fail(format, Exit::Failed, &error.to_string()),
        };
    }
    let _ = handle.respond(&request.key, builder.build()).await;

    transport
        .report(
            Report::new()
                .text("status", "refused")
                .text("caller", caller)
                .number("code", i64::from(code)),
            request.transport,
        )
        .emit(format);
    Exit::Success
}

/// A `To` for a final response, tagged if the request left it untagged.
///
/// RFC 3261 §8.2.6.2 has the UAS add a tag to every response except 100; a tag already
/// present names an existing dialog, whose tag is not ours to replace.
fn tagged(to: &str, tag: &str) -> String {
    if has_tag(to) {
        return to.to_owned();
    }
    format!("{to};tag={tag}")
}

/// Whether a `To` already carries a tag.
///
/// Only parameters after the closing bracket belong to the header; inside it they are the
/// URI's (RFC 3261 §20.10), and a URI parameter spelled `tag` does not identify a dialog.
fn has_tag(to: &str) -> bool {
    let params = to
        .rfind('>')
        .map_or(to, |end| to.get(end..).unwrap_or_default());
    params.split(';').skip(1).any(|param| {
        param
            .split('=')
            .next()
            .is_some_and(|name| name.trim().eq_ignore_ascii_case("tag"))
    })
}

/// A fresh tag value.
///
/// RFC 3261 §19.3 asks for global uniqueness and at least 32 bits of randomness; 64 random
/// bits give both without coordinating with anyone.
fn fresh_tag() -> String {
    use rand::Rng as _;
    let value: u64 = rand::rng().random();
    format!("{value:016x}")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn an_untagged_to_gains_a_tag() {
        assert_eq!(
            tagged("<sip:answer@example.com>", "abcd"),
            "<sip:answer@example.com>;tag=abcd"
        );
    }

    /// A tag already on the header names a dialog this response belongs to, and replacing it
    /// would move the response into a dialog that does not exist.
    #[test]
    fn a_to_that_already_has_a_tag_keeps_it() {
        assert_eq!(tagged("<sip:a@b>;tag=1", "x"), "<sip:a@b>;tag=1");
        assert_eq!(tagged("sip:a@b;tag=1", "x"), "sip:a@b;tag=1");
        assert_eq!(
            tagged("Bob <sip:a@b>;q=1;tag=1", "x"),
            "Bob <sip:a@b>;q=1;tag=1"
        );
    }

    /// RFC 3261 §20.10: inside the brackets, parameters belong to the URI. A `tag` there is
    /// not the header's, and the response still needs one.
    #[test]
    fn a_uri_parameter_spelled_tag_does_not_count() {
        assert_eq!(tagged("<sip:a@b;tag=1>", "x"), "<sip:a@b;tag=1>;tag=x");
    }

    /// RFC 3261 §19.3 asks for at least 32 bits of randomness; sixteen hex digits carry 64,
    /// and two draws that collide would mean they carried none.
    #[test]
    fn a_fresh_tag_is_long_enough_and_not_repeated() {
        let tag = fresh_tag();
        assert_eq!(tag.len(), 16);
        assert!(tag.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(tag, fresh_tag());
    }
}
