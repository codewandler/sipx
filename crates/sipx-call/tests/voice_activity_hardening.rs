//! Call-audio analysis under hostile lifecycles, on live calls (`M-61`).
//!
//! `sipx-audio`'s adversarial suite and the `call_audio_sequence` corpus prove what one analyser
//! does with hostile *audio*. What neither can prove is what a *call* does with a hostile
//! lifecycle: cancellation arriving while frames are in flight, detection replaced under load, an
//! application that stopped reading its own event stream, and two calls running at once.
//!
//! Both tests below tap the **outbound** direction. The seam takes transmitted audio after the mute
//! gate and before encoding (`docs/specs/call-audio-seam.md` §3), so what the analyser measures is
//! the sample the test wrote — which is what lets a sentinel be an exact number instead of a band
//! the codec happens to land in.
//!
//! Nothing here waits a fixed duration to establish an ordering. Every wait is a bound on failure
//! around an event: a report arriving, a transition arriving, a playback resolving.

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

use sipx_audio::analysis::{AnalysisProfile, AudioDirection, CalibrationProfile};
use sipx_audio::signal::{SignalReport, SignalReportProfile};
use sipx_call::{Call, CallEvent, CallEvents, answer, dial};
use sipx_sip::{Host, HostName, Uri};
use sipx_transport::{Config, Handle, Incoming, Target, bind};
use tokio::sync::mpsc::Receiver;

/// A bound on failure for audio crossing the media path, orders of magnitude above the honest
/// answer on an idle machine. Never a window anything is measured in: every number asserted below
/// comes from the samples, not from the clock.
const ARRIVAL_BOUND: Duration = Duration::from_secs(20);

/// `ALPHA`'s sentinel amplitude.
///
/// 30,011 is prime, is not full scale — so it clips nothing — and is far above the reference
/// profile's 2,048 activation amplitude, so alternating it is unambiguously voice. It is also not
/// any derived count of either profile below, which is what makes finding it in the *other* call's
/// report a fact and not a coincidence.
const ALPHA_AMPLITUDE: i16 = 30_011;

/// `BETA`'s sentinel amplitude.
///
/// 701 is prime, sits above the 64 silence floor and far below the 2,048 activation amplitude, so
/// alternating it is unambiguously *not* voice and unambiguously *not* silence. Disjoint from
/// [`ALPHA_AMPLITUDE`] by a factor of forty: no rounding, resampling or accumulation reaches from
/// one to the other.
const BETA_AMPLITUDE: i16 = 701;

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

/// Alternating `±amplitude`: peak and window deviation are both exactly `amplitude`, and the sum
/// over any even span is exactly zero, so every fact a report can carry is a stated number.
fn sentinel(amplitude: i16, samples: usize) -> Vec<i16> {
    (0..samples)
        .map(|index| {
            if index % 2 == 0 {
                amplitude
            } else {
                -amplitude
            }
        })
        .collect()
}

/// Every event this stream produces until `reports` signal reports have arrived, bounded so a
/// wiring mistake fails instead of hanging.
///
/// Everything is kept, not just the reports: a helper that consumed the transitions on its way past
/// them would make "no voice event arrived" true by construction.
async fn until_reports(events: &mut CallEvents, reports: usize) -> Vec<CallEvent> {
    let deadline = tokio::time::Instant::now() + ARRIVAL_BOUND;
    let mut seen = Vec::new();
    let mut counted = 0usize;
    while counted < reports {
        let event = tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("no timeout waiting for a signal report")
            .expect("the call's event stream ended before a report arrived");
        if let CallEvent::SignalMetrics(metrics) = &event
            && metrics.report().is_some()
        {
            counted += 1;
        }
        seen.push(event);
    }
    seen
}

/// The reports among a run of events, with the call each one named.
fn reports(events: &[CallEvent]) -> Vec<(String, SignalReport)> {
    events
        .iter()
        .filter_map(|event| match event {
            CallEvent::SignalMetrics(metrics) => metrics
                .report()
                .map(|report| (metrics.call_id().to_owned(), *report)),
            _ => None,
        })
        .collect()
}

/// Every report on one stream describes that call's own sentinel, exactly.
///
/// `peak` and `rms` are both the sentinel amplitude for an alternating `±v` stream, and `samples`
/// is the call's own derived window — so a report built from the other call's audio, or by the
/// other call's profile, fails here on a number rather than on a label.
fn assert_reports_are_the_sentinel(
    seen: &[(String, SignalReport)],
    id: &str,
    amplitude: i16,
    window_samples: u64,
    active: bool,
) {
    assert!(seen.len() >= 4, "{id} reported {} times", seen.len());
    for (call_id, report) in seen {
        assert_eq!(call_id, id, "a report named the wrong call");
        assert_eq!(report.peak, i32::from(amplitude), "{report:?}");
        assert_eq!(
            report.rms,
            u32::from(amplitude.unsigned_abs()),
            "{report:?}"
        );
        assert_eq!(report.samples, window_samples, "its own window: {report:?}");
        assert_eq!(
            report.active_windows,
            if active { report.windows } else { 0 },
            "measured against its own activation amplitude: {report:?}"
        );
        assert_eq!(report.clipping_windows, 0, "neither sentinel is full scale");
        assert_eq!(
            report.silent_windows, 0,
            "both sentinels are above the silence floor"
        );
    }
}

/// Every event that names a call names this one.
fn assert_every_event_names(seen: &[CallEvent], id: &str) {
    for event in seen {
        let named = match event {
            CallEvent::VoiceStarted(activity) | CallEvent::VoiceEnded { activity, .. } => {
                Some(activity.call_id().to_owned())
            }
            CallEvent::VoiceThresholds(thresholds) => Some(thresholds.call_id().to_owned()),
            CallEvent::SignalMetrics(metrics) => Some(metrics.call_id().to_owned()),
            _ => None,
        };
        if let Some(named) = named {
            assert_eq!(named, id, "an event named the wrong call: {event:?}");
        }
    }
}

/// Everything the stream is still holding, without waiting for anything.
fn remaining(events: &mut CallEvents) -> Vec<CallEvent> {
    let mut seen = Vec::new();
    while let Some(event) = events.try_recv() {
        seen.push(event);
    }
    seen
}

/// Two calls carrying deliberately incompatible audio at the same time: no state, no metric and no
/// event crosses between them.
///
/// The point of the two sentinels is that **swapping the streams fails every assertion**. `ALPHA`
/// carries alternating ±30,011 — a peak, an RMS and a window deviation of exactly 30,011, active by
/// a factor of fifteen, clipping nothing. `BETA` carries alternating ±701 — the same three numbers
/// at 701, which is above the silence floor and below the activation amplitude, so it is neither
/// voice nor silence. A report that named the other call's audio would have to carry a number from
/// the other call's alphabet, and the two alphabets are forty times apart.
///
/// Three independent dimensions are checked, because one would only prove the events are labelled:
/// the **metrics** (each call's exact sample facts), the **events** (voice opens on `ALPHA` and
/// never on `BETA`), and the **state** (each call's `voice_thresholds` reports its own configured
/// profile, which the two calls deliberately do not share).
#[tokio::test]
async fn two_calls_with_distinct_sentinel_streams_share_no_state_metric_or_event() {
    let (mut alpha, _alpha_callee) = connected().await;
    let (mut beta, _beta_callee) = connected().await;

    let rate = alpha.media().audio_rate();
    let per_packet = alpha.media().samples_per_packet();
    assert_eq!(
        rate,
        beta.media().audio_rate(),
        "one media format, two calls"
    );

    // Deliberately different profiles: a window, an activation amplitude and a calibration floor
    // each call does not share with the other, so a snapshot that came from the wrong analyser is
    // visible without looking at any audio.
    let alpha_profile = AnalysisProfile::new(AudioDirection::Outbound, rate)
        .with_window_ms(20)
        .with_activation_amplitude(2_048)
        .with_calibration(Some(CalibrationProfile::new().with_floor_amplitude(256)));
    let beta_profile = AnalysisProfile::new(AudioDirection::Outbound, rate)
        .with_window_ms(40)
        .with_activation_amplitude(1_024)
        .with_calibration(Some(
            CalibrationProfile::new()
                .with_floor_amplitude(512)
                .with_ceiling_amplitude(4_096),
        ));

    let mut alpha_events = alpha.events().expect("the stream is taken once");
    let mut beta_events = beta.events().expect("the stream is taken once");
    alpha
        .detect_voice_activity(alpha_profile)
        .await
        .expect("the seam accepts the attachment");
    beta.detect_voice_activity(beta_profile)
        .await
        .expect("the seam accepts the attachment");
    alpha
        .report_signal_metrics(SignalReportProfile::new(alpha_profile))
        .await
        .expect("the seam accepts the attachment");
    beta.report_signal_metrics(SignalReportProfile::new(beta_profile))
        .await
        .expect("the seam accepts the attachment");

    let alpha_id = String::from_utf8_lossy(&alpha.dialog.id.call_id).into_owned();
    let beta_id = String::from_utf8_lossy(&beta.dialog.id.call_id).into_owned();
    assert_ne!(alpha_id, beta_id, "two calls, two identities");

    // Both calls transmit at once, each its own sentinel.
    let alpha_clip = sentinel(ALPHA_AMPLITUDE, per_packet * 8);
    let beta_clip = sentinel(BETA_AMPLITUDE, per_packet * 8);
    let (alpha_played, beta_played) = tokio::join!(alpha.play(&alpha_clip), beta.play(&beta_clip));
    assert!(alpha_played && beta_played, "both clips ran to the end");

    // --- the metrics dimension: each call's exact sample facts, and only its own ---
    let mut alpha_seen = until_reports(&mut alpha_events, 4).await;
    let mut beta_seen = until_reports(&mut beta_events, 4).await;
    alpha_seen.extend(remaining(&mut alpha_events));
    beta_seen.extend(remaining(&mut beta_events));

    assert_reports_are_the_sentinel(
        &reports(&alpha_seen),
        &alpha_id,
        ALPHA_AMPLITUDE,
        u64::from(rate) / 50,
        true,
    );
    assert_reports_are_the_sentinel(
        &reports(&beta_seen),
        &beta_id,
        BETA_AMPLITUDE,
        u64::from(rate) / 25,
        false,
    );

    // --- the event dimension: voice opens on ALPHA and never on BETA ---
    assert!(
        alpha_seen
            .iter()
            .any(|event| matches!(event, CallEvent::VoiceStarted(_))),
        "ALPHA carried voice and was never told: {alpha_seen:?}"
    );
    assert!(
        !beta_seen
            .iter()
            .any(|event| matches!(event, CallEvent::VoiceStarted(_))),
        "BETA carried nothing above its own activation amplitude: {beta_seen:?}"
    );
    assert_every_event_names(&alpha_seen, &alpha_id);
    assert_every_event_names(&beta_seen, &beta_id);

    // --- the state dimension: each call measures against its own profile, not the other's ---
    let alpha_state = alpha.voice_thresholds().expect("detection is running");
    let beta_state = beta.voice_thresholds().expect("detection is running");
    assert_eq!(alpha_state.window_samples(), rate / 50);
    assert_eq!(beta_state.window_samples(), rate / 25);
    assert_eq!(alpha_state.profile().activation_amplitude(), 2_048);
    assert_eq!(beta_state.profile().activation_amplitude(), 1_024);
    assert_eq!(
        alpha_state
            .calibration()
            .map(CalibrationProfile::floor_amplitude),
        Some(256)
    );
    assert_eq!(
        beta_state
            .calibration()
            .map(CalibrationProfile::floor_amplitude),
        Some(512),
        "each call's bounds are its own"
    );
    // And neither analyser can have been more sensitive than the floor it declares — the one thing
    // a shared threshold would break first.
    assert!((256..=8_192).contains(&alpha_state.activation_amplitude()));
    assert!((512..=4_096).contains(&beta_state.activation_amplitude()));
}

/// Cancellation racing frame delivery, resets and a consumer that stopped reading leaves no
/// processor state, no task and no retained audio.
///
/// Three hostile things happen at once. Audio is in flight through the seam; detection is replaced
/// five times, and each replacement stops and *joins* the running watcher and re-attaches, which is
/// a reset of the analyser's epoch under load; and the application never reads its own event stream,
/// so every emission is refused for want of capacity.
///
/// The task half is checked by construction rather than by inspection. This runs on tokio's
/// current-thread runtime, so a watcher that kept looping over the media path after its session
/// stopped — `M-93`'s shape, where a long-running wait answers without suspending — would hold the
/// only worker there is and nothing after it would run at all, timer included. The probe at the end
/// is what turns that into a failure with a name.
#[tokio::test]
async fn cancelling_detection_while_frames_are_in_flight_leaves_no_state_task_or_audio() {
    let (mut caller, _callee) = connected().await;
    let rate = caller.media().audio_rate();
    let per_packet = caller.media().samples_per_packet();
    let profile = AnalysisProfile::new(AudioDirection::Outbound, rate)
        .with_calibration(Some(CalibrationProfile::new()));

    let mut events = caller.events().expect("the stream is taken once");
    caller
        .detect_voice_activity(profile)
        .await
        .expect("the seam accepts the attachment");
    caller
        .report_signal_metrics(SignalReportProfile::new(profile))
        .await
        .expect("the seam accepts the attachment");

    // Audio in flight, driven through the shared media handle so the call itself stays borrowable.
    let media = caller.media_handle();
    let clip = sentinel(ALPHA_AMPLITUDE, per_packet * 40);
    let playing = tokio::spawn(async move { media.play(&clip, per_packet).await });

    // Five replacements while those frames are being delivered. Each one stops and joins the
    // running watcher before attaching the next, so a watcher that could not be joined would wedge
    // here rather than leak quietly.
    for _ in 0..5u8 {
        caller
            .detect_voice_activity(profile)
            .await
            .expect("the seam accepts the replacement");
    }
    // The application has read nothing at all, so the event queue is full and every transition
    // since the first has been refused for want of capacity.
    assert!(
        caller.events().is_none(),
        "the stream is taken once, and this test is holding it"
    );

    caller.hang_up().await.expect("the call ends");
    let _ = playing.await;

    // --- no task ---
    // A watcher still looping over a stopped session would own this runtime's only worker, so this
    // would never resolve. The duration bounds that failure; it measures nothing.
    tokio::time::timeout(ARRIVAL_BOUND, async {
        for _ in 0..64u8 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the runtime still schedules work after teardown");

    // --- no processor ---
    // The session is stopped, so the seam refuses a new attachment. A processor that had outlived
    // the call would be holding a queue this answer says cannot exist.
    let refused = caller.media().attach_processor(sipx_media::Processing::new(
        AudioDirection::Outbound,
        sipx_audio::PcmFormat::new(rate, sipx_audio::PcmEncoding::Signed16).expect("valid"),
    ));
    assert!(
        refused.is_err(),
        "the seam handed out an attachment on a stopped session"
    );

    // --- no state, and the completion observation is last ---
    let tail = remaining(&mut events);
    let ended = tail
        .iter()
        .position(|event| matches!(event, CallEvent::Ended(_)));
    assert_eq!(
        ended,
        Some(tail.len() - 1),
        "`Ended` is the call's last word: {tail:?}"
    );
    let mut open = false;
    for event in &tail {
        match event {
            CallEvent::VoiceStarted(_) => open = true,
            CallEvent::VoiceEnded { .. } => open = false,
            _ => {}
        }
    }
    assert!(!open, "teardown left voice latched open: {tail:?}");

    // --- no retained audio ---
    // Every event a call can carry is counters and amplitudes; none of them is a buffer. The
    // rendering is swept for the sentinel the call was full of, and for a rendered slice of any
    // kind.
    for event in &tail {
        let record = format!("{event:?}");
        assert!(
            !record.contains("30011, 30011"),
            "an event carried a run of samples: {record}"
        );
        assert!(
            !record.contains("samples: ["),
            "an event carried a sample buffer: {record}"
        );
    }

    // The analyser's in-progress measurement is gone: the terminal reset discarded the period and
    // re-armed the warm-up (`docs/specs/call-audio-processing.md` §12.8), so what an application can
    // read after teardown is that and not a stale one.
    let after = caller
        .voice_thresholds()
        .expect("the call still remembers what it was measuring against");
    assert_eq!(after.observed_floor(), None, "the period was discarded");
    assert_eq!(after.outcome(), None, "the warm-up was re-armed");
    assert_eq!(after.frozen_samples(), 0);
}
