//! `sipx answer`.

use std::time::Duration;

use sipx_sip::{HeaderName, StatusCode};
use sipx_transport::{Config as TransportConfig, bind};

use crate::cli::AnswerOptions;
use crate::output::{Exit, Format, Report, fail};

#[allow(
    clippy::too_many_lines,
    reason = "the command lifecycle is kept in execution order so validation-before-I/O remains auditable"
)]
pub(crate) async fn run(options: AnswerOptions, format: Format) -> Exit {
    let signalling = options.signalling.complete();
    let transport = match crate::signalling::Selection::from_options(&signalling, false) {
        Ok(transport) => transport,
        Err(message) => return fail(format, Exit::Usage, &message),
    };
    let media = match crate::media::Selection::from_options(&options.media, transport.kind(), false)
    {
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

    let clip = match audio.wav_input().map(crate::dial::read_clip) {
        Some(Ok(clip)) => Some(clip),
        Some(Err(message)) => return fail(format, Exit::Usage, &message),
        None => None,
    };
    let mut devices = match audio.open() {
        Ok(devices) => devices,
        Err(message) => return fail(format, Exit::Failed, &message),
    };

    let local = options.local;
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

    let mut config = TransportConfig::new(local);
    if let Some(advertised) = advertised {
        config.sent_by = advertised.to_string();
    } else if local.ip().is_unspecified() {
        "127.0.0.1".clone_into(&mut config.sent_by);
    }
    crate::apply_capture(&options.capture, &mut config);
    if let Err(message) = transport.configure_listener(&signalling, &mut config) {
        return fail(format, Exit::Usage, &message);
    }
    let (handle, mut incoming) = match bind(config).await {
        Ok(bound) => bound,
        Err(error) => return fail(format, Exit::Failed, &format!("bind: {error}")),
    };

    // Armed here, not at the end: the run that most needs the numbers is the one that fails, and
    // every `return fail(…)` below now takes the counters file with it.
    let export = crate::counters::Export::arm(&options.capture, &handle);

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

    let wait = Duration::from_secs(options.wait);
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

    if options.reject || options.busy {
        return refuse(
            &handle,
            &request,
            &caller,
            options.busy,
            transport,
            format,
            &headers,
        )
        .await;
    }

    let media_addresses = crate::advertise::media_addresses(local, request.source.ip(), advertised);
    let media_address = media_addresses.advertised;

    let started = std::time::Instant::now();
    let mut call = match sipx_call::answer_with_policy_and_headers_at(
        &handle,
        &request,
        sipx_call::MediaAddress::new(media_address).with_bind(media_addresses.bind),
        media.policy(),
        &headers,
    )
    .await
    {
        Ok(call) => call,
        Err(error) => return fail(format, Exit::Failed, &error.to_string()),
    };

    let duration = Duration::from_secs(options.duration);
    let session = call.media();
    let pcm = match clip.as_ref().map(crate::dial::pcm_clip).transpose() {
        Ok(pcm) => pcm,
        Err(message) => {
            let _ = call.hang_up().await;
            return fail(format, Exit::Usage, &message);
        }
    };
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
            if let Some(pcm) = &pcm {
                let _ = tokio::time::timeout(duration, session.play_pcm(pcm)).await;
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
            .text("media_advertised", media_address.to_string())
            .text("media_bound", call.media().local_addr().to_string())
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
    let report = media.negotiated_report(report, &call, "browser-answerer");
    let mut report = devices.report(transport.report(report, request.transport));

    if !digits.is_empty() {
        report = report.text("dtmf", digits);
    }

    if let Some(path) = audio.wav_output() {
        match crate::dial::write_clip(path, &recorded, session.clock_rate()) {
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
