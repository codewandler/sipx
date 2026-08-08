//! Signal metrics on a live call (`M-59`): level, clipping and silence reported as typed
//! [`CallEvent`]s over `M-54`'s one call-media seam.
//!
//! These run against two connected calls on loopback rather than against the reporter in
//! isolation, because what is worth proving here is the *carriage*: that the numbers a call
//! reports describe the audio that call actually carried, that they name the call, direction,
//! epoch, sequence, sample position and window coverage they were measured over, and that two
//! calls running at once never report each other's audio.
//!
//! The arithmetic is pinned by `sipx-audio`'s `signal_metrics.rs` against
//! `docs/specs/call-audio-processing.md` §11, and the reporting policy by this module's own unit
//! tests. Nothing is re-derived here.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
// `caller`/`callee` and their endpoints are the vocabulary every other call test in this crate
// uses; renaming them here to satisfy a similarity heuristic would make this the odd one out.
#![allow(clippy::similar_names)]

use std::net::IpAddr;
use std::time::Duration;

use sipx_audio::analysis::{AnalysisProfile, AudioDirection};
use sipx_audio::signal::{SignalObservation, SignalReport, SignalReportProfile};
use sipx_call::{Call, CallEvent, CallEvents, answer, dial};
use sipx_sip::{Host, HostName, Uri};
use sipx_transport::{Config, Handle, Incoming, Target, bind};
use tokio::sync::mpsc::Receiver;

/// A bound on failure for audio crossing loopback, orders of magnitude above the honest answer on
/// an idle machine. Never a window anything is measured in — every number asserted below comes
/// from the samples, not from the clock.
const ARRIVAL_BOUND: Duration = Duration::from_secs(10);

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("valid")
}

async fn endpoint() -> (Handle, Receiver<Incoming>) {
    bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds")
}

fn callee_uri() -> Uri {
    Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")))
}

async fn connected() -> (Call, Call) {
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, _caller_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    let answering = tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("an INVITE arrives");
        answer(&callee_endpoint, &incoming, loopback())
            .await
            .expect("answers")
    });

    let caller = dial(
        &caller_endpoint,
        Target::udp(callee_addr),
        &callee_uri(),
        &sipx_call::DialOptions::new("<sip:caller@example.net>", loopback()),
    )
    .await
    .expect("the call connects");

    let callee = answering.await.expect("the answering side finishes");
    (caller, callee)
}

/// One window of transmitted audio per report, at the session's own rate.
fn outbound(rate: u32) -> SignalReportProfile {
    SignalReportProfile::new(
        AnalysisProfile::new(AudioDirection::Outbound, rate).with_window_ms(20),
    )
}

/// The next signal report on this stream, bounded so a wiring mistake fails instead of hanging.
async fn next_report(events: &mut CallEvents) -> (String, AudioDirection, SignalReport) {
    let deadline = tokio::time::Instant::now() + ARRIVAL_BOUND;
    loop {
        let event = tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("no timeout waiting for a signal report")
            .expect("the call's event stream ended before a report arrived");
        if let CallEvent::SignalMetrics(metrics) = event
            && let Some(report) = metrics.report()
        {
            return (metrics.call_id().to_owned(), metrics.direction(), *report);
        }
    }
}

/// The story's failing-first case: a call reports the level, clipping and coverage of the audio it
/// actually carried, as a typed event on its own stream.
#[tokio::test]
async fn a_call_reports_the_level_and_clipping_of_the_audio_it_carried() {
    let (mut caller, _callee) = connected().await;
    let mut events = caller.events().expect("the stream is taken once");

    let rate = caller.media().audio_rate();
    let per_packet = caller.media().samples_per_packet();
    caller
        .report_signal_metrics(outbound(rate))
        .await
        .expect("the seam accepts the attachment");

    // Full scale, so the expected facts are unambiguous: every sample clips, the peak is the
    // largest positive magnitude, and a constant signal has no variance and is therefore not
    // activity.
    assert!(caller.play(&vec![i16::MAX; per_packet * 4]).await);

    let (call_id, direction, report) = next_report(&mut events).await;

    assert!(!call_id.is_empty(), "the report names its own call");
    assert_eq!(direction, AudioDirection::Outbound);
    assert_eq!(report.rate, rate);
    assert_eq!(report.epoch, 0, "the first epoch of this call");
    assert_eq!(report.sequence, 0, "the first report of that epoch");
    assert_eq!(report.first_window, 0);
    assert_eq!(report.windows, 1);
    assert_eq!(report.at_sample, 0);
    assert_eq!(report.samples, u64::from(rate) / 50, "one 20 ms window");

    assert_eq!(report.peak, i32::from(i16::MAX));
    assert_eq!(report.clipped_samples, report.samples);
    assert_eq!(report.clipping_windows, 1);
    assert_eq!(report.silent_windows, 0);
    assert_eq!(
        report.active_windows, 0,
        "a constant full-scale signal has variance zero and is not activity"
    );
    assert_eq!(report.rms, u32::try_from(report.peak).unwrap());
}

/// Coverage is contiguous and monotonic: consecutive reports name consecutive windows and the
/// sample positions that follow from them, so an application places every fact on a timeline
/// without knowing the cadence.
#[tokio::test]
async fn consecutive_reports_cover_consecutive_windows_without_a_gap() {
    let (mut caller, _callee) = connected().await;
    let mut events = caller.events().expect("the stream is taken once");

    let rate = caller.media().audio_rate();
    let per_packet = caller.media().samples_per_packet();
    caller
        .report_signal_metrics(outbound(rate))
        .await
        .expect("attaches");

    assert!(caller.play(&vec![1_000i16; per_packet * 6]).await);

    let (_, _, first) = next_report(&mut events).await;
    let (_, _, second) = next_report(&mut events).await;

    assert_eq!(second.epoch, first.epoch, "one uninterrupted measurement");
    assert_eq!(second.sequence, first.sequence + 1);
    assert_eq!(second.first_window, first.first_window + 1);
    assert_eq!(second.at_sample, first.at_sample + first.samples);
    assert_eq!(second.peak, 1_000);
    assert_eq!(second.dc_offset_windows, 1, "a constant offset is DC");
}

/// The cadence is what bounds a call's event rate: four windows of audio, one event.
#[tokio::test]
async fn the_reporting_cadence_bounds_the_events_a_call_produces() {
    let (mut caller, _callee) = connected().await;
    let mut events = caller.events().expect("the stream is taken once");

    let rate = caller.media().audio_rate();
    let per_packet = caller.media().samples_per_packet();
    caller
        .report_signal_metrics(outbound(rate).with_windows_per_report(4))
        .await
        .expect("attaches");

    assert!(caller.play(&vec![i16::MAX; per_packet * 8]).await);

    let (_, _, report) = next_report(&mut events).await;
    assert_eq!(report.windows, 4);
    assert_eq!(report.samples, (u64::from(rate) / 50) * 4);
    assert_eq!(report.clipping_windows, 4);
    assert_eq!(report.first_window, 0);
}

/// Two calls at once: each reports its own audio and nothing of the other's, and each report says
/// which call it came from.
#[tokio::test]
async fn two_calls_never_report_each_others_audio() {
    let (mut loud, _loud_callee) = connected().await;
    let (mut quiet, _quiet_callee) = connected().await;

    let mut loud_events = loud.events().expect("the stream is taken once");
    let mut quiet_events = quiet.events().expect("the stream is taken once");

    let rate = loud.media().audio_rate();
    let per_packet = loud.media().samples_per_packet();
    loud.report_signal_metrics(outbound(rate))
        .await
        .expect("attaches");
    quiet
        .report_signal_metrics(outbound(rate))
        .await
        .expect("attaches");

    assert!(loud.play(&vec![i16::MAX; per_packet * 4]).await);
    assert!(quiet.play(&vec![0i16; per_packet * 4]).await);

    let (loud_id, _, loud_report) = next_report(&mut loud_events).await;
    assert_eq!(loud_report.peak, i32::from(i16::MAX));
    assert_eq!(loud_report.silent_windows, 0);

    let (quiet_id, _, quiet_report) = next_report(&mut quiet_events).await;
    assert_eq!(quiet_report.peak, 0);
    assert_eq!(quiet_report.silent_windows, 1);
    assert_eq!(quiet_report.clipped_samples, 0);

    assert_ne!(loud_id, quiet_id, "two calls, two identities");
}

/// A profile outside the declared domains is refused before anything is attached, and the call
/// carries on reporting under the profile it already had.
#[tokio::test]
async fn a_refused_profile_leaves_running_reporting_running() {
    let (mut caller, _callee) = connected().await;
    let mut events = caller.events().expect("the stream is taken once");

    let rate = caller.media().audio_rate();
    let per_packet = caller.media().samples_per_packet();
    caller
        .report_signal_metrics(outbound(rate))
        .await
        .expect("attaches");

    assert!(
        caller
            .report_signal_metrics(outbound(rate).with_windows_per_report(0))
            .await
            .is_err(),
        "a report over no windows is not a measurement"
    );
    assert!(
        caller
            .report_signal_metrics(SignalReportProfile::new(AnalysisProfile::new(
                AudioDirection::Outbound,
                0,
            )))
            .await
            .is_err(),
        "rate 0 is outside the linear-PCM domain"
    );

    assert!(caller.play(&vec![i16::MAX; per_packet * 2]).await);
    let (_, _, report) = next_report(&mut events).await;
    assert_eq!(report.clipping_windows, 1, "the first profile still runs");
}

/// Voice-activity detection and metric reporting are independent, and both may run at once on one
/// call without either one's analyser touching the other's.
#[tokio::test]
async fn voice_detection_and_metric_reporting_run_side_by_side() {
    let (mut caller, _callee) = connected().await;
    let mut events = caller.events().expect("the stream is taken once");

    let rate = caller.media().audio_rate();
    let per_packet = caller.media().samples_per_packet();
    caller
        .detect_voice_activity(AnalysisProfile::new(AudioDirection::Outbound, rate))
        .await
        .expect("voice detection attaches");
    caller
        .report_signal_metrics(outbound(rate))
        .await
        .expect("metric reporting attaches beside it");

    // Full-swing modulation: `active` by the variance predicate, and unmistakable in the level.
    let modulated: Vec<i16> = (0..per_packet * 4)
        .map(|index| if index % 2 == 0 { 8_192 } else { -8_192 })
        .collect();
    assert!(caller.play(&modulated).await);

    let (_, _, report) = next_report(&mut events).await;
    assert_eq!(report.peak, 8_192);
    assert_eq!(report.active_windows, 1);
    assert_eq!(report.clipping_windows, 0);
}

/// Ending the call stops reporting before `Ended`, which the stream promises is its last event.
#[tokio::test]
async fn no_report_arrives_after_the_call_has_ended() {
    let (mut caller, _callee) = connected().await;
    let mut events = caller.events().expect("the stream is taken once");

    let rate = caller.media().audio_rate();
    let per_packet = caller.media().samples_per_packet();
    caller
        .report_signal_metrics(outbound(rate))
        .await
        .expect("attaches");

    assert!(caller.play(&vec![i16::MAX; per_packet * 2]).await);
    let (_, _, report) = next_report(&mut events).await;
    assert_eq!(report.clipping_windows, 1);

    caller.hang_up().await.expect("hangs up");

    let mut seen_end = false;
    while let Some(event) = events.try_recv() {
        assert!(!seen_end, "an event arrived after Ended: {event:?}");
        seen_end = matches!(event, CallEvent::Ended(_));
    }
    assert!(seen_end, "the call reported that it ended");
}

/// A silence transition reaches the application on a live call, and a silent stretch is reported
/// as silent rather than as absent.
#[tokio::test]
async fn a_silent_call_reports_silence() {
    let (mut caller, _callee) = connected().await;
    let mut events = caller.events().expect("the stream is taken once");

    let rate = caller.media().audio_rate();
    let per_packet = caller.media().samples_per_packet();
    caller
        .report_signal_metrics(outbound(rate))
        .await
        .expect("attaches");

    assert!(caller.play(&vec![0i16; per_packet * 4]).await);

    let (_, _, report) = next_report(&mut events).await;
    assert_eq!(report.peak, 0);
    assert_eq!(report.silent_windows, 1);
    assert_eq!(report.rms, 0);
    assert!(
        !matches!(
            SignalObservation::Report(report),
            SignalObservation::SilenceElapsed { .. }
        ),
        "a report is not a transition"
    );
}
