//! Signal metrics on a live call (`M-59`): level, clipping and silence reported as typed
//! [`CallEvent`]s over `M-54`'s one call-media seam.
//!
//! These run against two connected calls on loopback rather than against the processor in
//! isolation, because what is worth proving here is the *carriage*: that the numbers a call
//! reports describe the audio that call actually carried, that they name the direction, epoch,
//! sequence, sample position and window coverage they were measured over, and that two calls
//! running at once never report each other's samples.
//!
//! The arithmetic itself is pinned by `sipx-audio`'s `signal_metrics.rs` vectors against
//! `docs/specs/call-audio-processing.md` §11; nothing is re-derived here.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::IpAddr;
use std::time::Duration;

use sipx_audio::signal::{
    SignalDirection, SignalObservation, SignalProfile, SignalReport,
};
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

/// A profile that measures one packet's worth of transmitted audio per window.
fn outbound_profile(rate: u32) -> SignalProfile {
    SignalProfile::new(SignalDirection::Outbound, rate)
        .with_window_ms(20)
        .with_clip_samples(8)
        .with_windows_per_report(1)
}

/// The next signal report on this stream, bounded so a wiring mistake fails instead of hanging.
async fn next_report(events: &mut CallEvents) -> (SignalDirection, SignalReport) {
    let deadline = tokio::time::Instant::now() + ARRIVAL_BOUND;
    loop {
        let event = tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("no timeout waiting for a signal report")
            .expect("the call's event stream ended before a report arrived");
        if let CallEvent::SignalMetrics {
            direction,
            observation: SignalObservation::Report(report),
        } = event
        {
            return (direction, report);
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
    let _observer = caller
        .observe_signal_metrics(outbound_profile(rate))
        .expect("the seam accepts one observer");

    // Full scale, so the expected facts are unambiguous: every sample clips, the peak is the
    // largest positive magnitude, and a constant signal has no variance and is therefore not
    // activity.
    caller.play(&vec![i16::MAX; per_packet * 4]).await;

    let (direction, report) = next_report(&mut events).await;

    assert_eq!(direction, SignalDirection::Outbound);
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
/// sample positions that follow from them, so an application can place every fact on a timeline
/// without knowing the cadence.
#[tokio::test]
async fn consecutive_reports_cover_consecutive_windows_without_a_gap() {
    let (mut caller, _callee) = connected().await;
    let mut events = caller.events().expect("the stream is taken once");

    let rate = caller.media().audio_rate();
    let per_packet = caller.media().samples_per_packet();
    let _observer = caller
        .observe_signal_metrics(outbound_profile(rate))
        .expect("the seam accepts one observer");

    caller.play(&vec![1_000i16; per_packet * 6]).await;

    let (_, first) = next_report(&mut events).await;
    let (_, second) = next_report(&mut events).await;

    assert_eq!(second.epoch, first.epoch, "one uninterrupted measurement");
    assert_eq!(second.sequence, first.sequence + 1);
    assert_eq!(second.first_window, first.first_window + 1);
    assert_eq!(second.at_sample, first.at_sample + first.samples);
    assert_eq!(second.peak, 1_000);
    assert_eq!(second.dc_offset_windows, 1, "a constant offset is DC");
}

/// Two calls at once: each observer reports its own call's audio and nothing of the other's.
///
/// The per-call isolation the epic requires, proved by giving the two calls signals that cannot be
/// confused — full scale against silence — and asserting neither ever sees the other's.
#[tokio::test]
async fn two_calls_never_report_each_others_samples() {
    let (mut loud, _loud_callee) = connected().await;
    let (mut quiet, _quiet_callee) = connected().await;

    let mut loud_events = loud.events().expect("the stream is taken once");
    let mut quiet_events = quiet.events().expect("the stream is taken once");

    let rate = loud.media().audio_rate();
    let per_packet = loud.media().samples_per_packet();
    let _loud_observer = loud
        .observe_signal_metrics(outbound_profile(rate))
        .expect("attaches");
    let _quiet_observer = quiet
        .observe_signal_metrics(outbound_profile(rate).with_silence_amplitude(64))
        .expect("attaches");

    loud.play(&vec![i16::MAX; per_packet * 4]).await;
    quiet.play(&vec![0i16; per_packet * 4]).await;

    for _ in 0..2 {
        let (_, report) = next_report(&mut loud_events).await;
        assert_eq!(report.peak, i32::from(i16::MAX));
        assert_eq!(report.silent_windows, 0);
    }
    for _ in 0..2 {
        let (_, report) = next_report(&mut quiet_events).await;
        assert_eq!(report.peak, 0);
        assert_eq!(report.silent_windows, 1);
        assert_eq!(report.clipped_samples, 0);
    }
}

/// Stopping an observer is an event, not a duration: `stop` returns when the attachment is
/// released, and nothing further is reported for it.
#[tokio::test]
async fn stopping_an_observer_completes_and_ends_the_reporting() {
    let (mut caller, _callee) = connected().await;
    let mut events = caller.events().expect("the stream is taken once");

    let rate = caller.media().audio_rate();
    let per_packet = caller.media().samples_per_packet();
    let observer = caller
        .observe_signal_metrics(outbound_profile(rate))
        .expect("attaches");

    caller.play(&vec![i16::MAX; per_packet * 2]).await;
    let (_, first) = next_report(&mut events).await;
    assert_eq!(first.clipping_windows, 1);
    assert_eq!(
        observer.refused_frames(),
        0,
        "the seam's own sequencing is exactly what the analyser accepts"
    );

    // `stop` joins the observer's task, so every event it will ever emit has been emitted by the
    // time this returns. Draining here therefore leaves a *final* state: anything that turned up
    // afterwards would have had to be emitted by a task that has already finished.
    observer.stop().await;
    while events.try_recv().is_some() {}

    assert!(caller.play(&vec![i16::MAX; per_packet * 4]).await);
    let mut seen = 0usize;
    while let Some(event) = events.try_recv() {
        seen += 1;
        assert!(
            !matches!(event, CallEvent::SignalMetrics { .. }),
            "a stopped observer must report nothing further: {event:?}"
        );
    }
    assert!(
        seen > 0,
        "the playback's own completion event proves the stream is still live"
    );
}

/// A profile outside the specification's domains is refused, and the call carries on.
#[tokio::test]
async fn a_refused_profile_leaves_the_call_running() {
    let (mut caller, _callee) = connected().await;
    let mut events = caller.events().expect("the stream is taken once");

    let rate = caller.media().audio_rate();
    let per_packet = caller.media().samples_per_packet();

    assert!(
        caller
            .observe_signal_metrics(outbound_profile(rate).with_window_ms(0))
            .is_err(),
        "a zero window is not a measurement"
    );
    assert!(
        caller
            .observe_signal_metrics(outbound_profile(rate).with_queue_capacity(1))
            .is_err(),
        "a queue below the declared minimum is refused"
    );

    let _observer = caller
        .observe_signal_metrics(outbound_profile(rate))
        .expect("a valid profile still attaches");
    caller.play(&vec![i16::MAX; per_packet * 2]).await;
    let (_, report) = next_report(&mut events).await;
    assert_eq!(report.clipping_windows, 1);
}

/// Both directions can be observed at once, and each names itself.
#[tokio::test]
async fn both_directions_can_be_observed_at_once() {
    let (mut caller, _callee) = connected().await;
    let mut events = caller.events().expect("the stream is taken once");

    let rate = caller.media().audio_rate();
    let per_packet = caller.media().samples_per_packet();
    let _outbound = caller
        .observe_signal_metrics(outbound_profile(rate))
        .expect("attaches to transmitted audio");
    let _inbound = caller
        .observe_signal_metrics(
            outbound_profile(rate)
                .with_window_ms(20)
                .with_windows_per_report(1),
        )
        .expect("a second attachment is within the seam's bound");

    caller.play(&vec![i16::MAX; per_packet * 2]).await;

    let (direction, report) = next_report(&mut events).await;
    assert_eq!(direction, SignalDirection::Outbound);
    assert_eq!(report.peak, i32::from(i16::MAX));
}
