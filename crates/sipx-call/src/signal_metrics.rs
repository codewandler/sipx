//! Signal metrics as typed call events (`M-59`).
//!
//! `sipx-audio`'s [`AudioAnalyzer`] turns PCM frames into deterministic observations
//! ([`docs/specs/call-audio-processing.md`](../../../docs/specs/call-audio-processing.md)), its
//! [`signal`](sipx_audio::signal) module shapes the level, clipping and silence half of those into
//! bounded reports (§10), and `sipx-media`'s one seam
//! ([`docs/specs/call-audio-seam.md`](../../../docs/specs/call-audio-seam.md)) is where live call
//! audio leaves the media path. This module is the join: it attaches **through that seam**, feeds
//! the analyser, and reports what the reducer yields on the call's own event stream as
//! [`CallEvent::SignalMetrics`].
//!
//! It is the sibling of [`crate::voice`] and works the same way on purpose — same seam, same
//! analyser contract, same call-owned lifecycle, same bounded delivery. The two differ in exactly
//! one thing, and it is the reason this module is shorter: **a metric is not a latched state.**
//! Voice activity has to retry an undelivered transition, because a dropped `VoiceStarted` beside
//! a delivered `VoiceEnded` would tell an application the opposite of what happened. A report is a
//! fact about a stretch of audio that has already gone by; a consumer that misses one has lost
//! history, not correctness, and the next report is not a correction of it. So reports are emitted
//! through the ordinary `Emitter` and a slow consumer's losses are counted by
//! [`CallEvents::dropped`](crate::CallEvents::dropped), with no reserved slot and no terminal
//! event.
//!
//! # Not a quality report
//!
//! Loss, jitter, round-trip time and the MOS estimate are `M-10`'s RTP/RTCP snapshot
//! ([`sipx_media::MediaSession::quality`]) and describe *delivery*. These describe *content*.
//! Nothing here measures packet delivery and no field of that snapshot changes meaning because
//! this exists.
//!
//! # Bounded end to end
//!
//! The seam's offer never waits (its §6.2), the analyser's observation queue is a fixed ring that
//! coalesces overflow into a counted marker (§8.3), the reporting cadence bounds how many events a
//! second of audio can produce, and emission is the call event stream's `try_send`. Nothing on
//! this path can block RTP decode, RTP encode, playback or capture, and nothing on it grows with
//! call duration.

use std::sync::Arc;

use sipx_audio::PcmSamples;
use sipx_audio::analysis::{AnalysisFrame, AudioAnalyzer, DiscontinuityKind};
use sipx_media::{PcmFrame, PcmProcessor};

use crate::event::{CallEvent, Emitter};

/// The vocabulary a signal-metric event is written in, re-exported from the modules that define
/// it.
///
/// Re-exported rather than restated, for the reason [`crate::voice`] gives: a second spelling of
/// "inbound", or of what a reset means, is how two layers of one stack start disagreeing about
/// what happened.
pub use sipx_audio::analysis::{AudioDirection, ResetCause};
pub use sipx_audio::signal::{
    MAX_WINDOWS_PER_REPORT, SignalObservation, SignalProfileError, SignalReport,
    SignalReportProfile,
};

/// One signal-metric observation, placed on one call's audio timeline.
///
/// Carries what an application needs to act on it without asking anything else: which call it
/// belongs to, which side of it was measured, and the observation itself — which names its own
/// epoch, rate, report sequence, first sample and window coverage. A host merging several calls'
/// event streams into one queue can still tell them apart and still order them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalMetrics {
    call_id: Arc<str>,
    direction: AudioDirection,
    observation: SignalObservation,
}

impl SignalMetrics {
    /// The `Call-ID` of the call this observation belongs to (RFC 3261 §8.1.1.4).
    #[must_use]
    pub fn call_id(&self) -> &str {
        &self.call_id
    }

    /// Which side of the call was measured.
    #[must_use]
    pub const fn direction(&self) -> AudioDirection {
        self.direction
    }

    /// The observation: a completed report, a silence transition, a reset that opened a new
    /// measurement epoch, or the counted marker for observations the analyser's queue lost.
    #[must_use]
    pub const fn observation(&self) -> &SignalObservation {
        &self.observation
    }

    /// The completed report, for the common case of an application that wants only those.
    #[must_use]
    pub const fn report(&self) -> Option<&SignalReport> {
        match &self.observation {
            SignalObservation::Report(report) => Some(report),
            _ => None,
        }
    }
}

/// Turns one seam attachment's frames into this call's signal-metric events.
///
/// Sans-I/O like the analyser and the reducer it owns: it reads no clock and awaits nothing, so
/// the whole reporting policy is exercised by feeding it frames.
#[derive(Debug)]
pub(crate) struct SignalMetricsReporter {
    analyzer: AudioAnalyzer,
    reporter: sipx_audio::signal::SignalReporter,
    call_id: Arc<str>,
    emitter: Emitter,
    /// The frame number handed to the analyser, which counts only frames it accepted. The seam's
    /// own sequence is checked below and then left behind: a frame this module declines to forward
    /// must not derange the analyser's sequence expectation.
    fed: u64,
    /// The last seam sequence seen, for detecting a gap the seam failed to flag.
    seam_sequence: Option<u64>,
    /// A break owed to the analyser before the next frame's samples are measured.
    ///
    /// Set when audio that should have been measured was not — a frame this module could not
    /// forward, or one the analyser refused. Carrying it forward is what keeps the *quantitative*
    /// promise of a report: a period that spanned the hole would name coverage it never measured,
    /// which is a worse answer than restarting the epoch and saying so.
    owed: Option<DiscontinuityKind>,
}

impl SignalMetricsReporter {
    pub(crate) fn new(
        analyzer: AudioAnalyzer,
        reporter: sipx_audio::signal::SignalReporter,
        call_id: Arc<str>,
        emitter: Emitter,
    ) -> Self {
        Self {
            analyzer,
            reporter,
            call_id,
            emitter,
            fed: 0,
            seam_sequence: None,
            owed: None,
        }
    }

    /// Feed one frame the seam delivered.
    pub(crate) fn observe(&mut self, frame: &PcmFrame) {
        let PcmSamples::Signed16(samples) = frame.pcm().samples() else {
            // discard: unreachable through `Call`, which always attaches at signed 16-bit — the
            // depth the analyser's contract is written in. Converting here instead would make this
            // a second place where call audio is reinterpreted. The audio is still owed to the
            // analyser, so the next frame carries the break rather than a report claiming to have
            // measured samples that never reached it.
            self.owed = Some(DiscontinuityKind::Loss);
            return;
        };
        self.observe_samples(
            frame.direction(),
            frame.sequence(),
            frame.discontinuity().map(sipx_media::Discontinuity::kind),
            samples,
        );
    }

    /// The body of [`Self::observe`], reachable without a live media session.
    fn observe_samples(
        &mut self,
        direction: AudioDirection,
        seam_sequence: u64,
        discontinuity: Option<DiscontinuityKind>,
        samples: &[i16],
    ) {
        if samples.is_empty() {
            // discard: an empty frame measures nothing and the analyser refuses one by contract
            // (§7.3). Nothing is owed, because nothing was lost.
            return;
        }

        // The seam counts every frame it offered, delivered or dropped, and always flags a gap
        // (seam §4). An unflagged gap is a defect in the seam rather than a loss to smooth over,
        // so it is named `Loss` here instead of being handed to the analyser as a legal stream —
        // which is what stops a report claiming coverage of windows nobody measured.
        // A break owed from a frame that never reached the analyser is named before anything the
        // seam declared, and the more disruptive of the two wins — the seam's vocabulary is closed
        // and `Realign` subsumes a span (its §7).
        let mut discontinuity = match (discontinuity, self.owed.take()) {
            (Some(declared), Some(DiscontinuityKind::Loss)) => Some(declared),
            (declared, owed) => declared.or(owed),
        };
        let unflagged_gap = discontinuity.is_none()
            && self
                .seam_sequence
                .is_some_and(|previous| seam_sequence > previous.saturating_add(1));
        if unflagged_gap {
            tracing::debug!(
                seam_sequence,
                "the call PCM seam skipped a frame without flagging it"
            );
            discontinuity = Some(DiscontinuityKind::Loss);
        }
        self.seam_sequence = Some(seam_sequence);

        let mut frame = AnalysisFrame::new(direction, self.fed, samples);
        if let Some(kind) = discontinuity {
            frame = frame.with_discontinuity(kind);
        }
        if let Err(refusal) = self.analyzer.process(&frame) {
            // discard: the seam's contract makes every refusal here a defect in this join rather
            // than in the caller's audio, so it is reported and the stream continues at the next
            // frame — carrying the break these unmeasured samples left, so no later report can sum
            // across the hole and claim coverage of them.
            tracing::debug!(%refusal, "the call audio analyser refused a seam frame");
            self.owed = Some(DiscontinuityKind::Loss);
            return;
        }
        self.fed = self.fed.saturating_add(1);

        self.report(direction);
    }

    /// Reduce whatever the analyser produced and emit what the reducer completed.
    ///
    /// The window length is read from the analyser *after* it processed the frame, so a format
    /// change would be reflected in the same call that reported it. Through `Call` no format
    /// change is ever declared — a media-session replacement re-attaches instead (`M-58`'s
    /// pattern), and an in-session clock change arrives as a `Realign` — so in practice this is
    /// constant for the attachment's life.
    fn report(&mut self, direction: AudioDirection) {
        let window = self.analyzer.window_samples();
        let mut produced: Vec<SignalObservation> = Vec::new();
        for observation in self.analyzer.drain() {
            // Voice transitions are `M-58`'s reading of the same windows and are not signal
            // metrics; the reducer returns `None` for them.
            if let Some(signal) = self.reporter.observe(&observation, window) {
                produced.push(signal);
            }
        }
        for signal in produced {
            self.emitter.emit(CallEvent::SignalMetrics(SignalMetrics {
                call_id: Arc::clone(&self.call_id),
                direction,
                observation: signal,
            }));
        }
    }
}

/// Drive one attachment until the call's audio is finished, or until the call stops it.
///
/// Completion is an event, not a duration: [`PcmProcessor::recv`] resolving to `None` is the seam
/// saying the session stopped or the attachment was released, so nothing here waits a fixed time
/// to learn the call is over.
///
/// Nothing is flushed at the end. A reporting period cut short by teardown covers fewer windows
/// than it would claim, and the specification's §5.2 rule for a partial *window* is the same rule
/// for the same reason: a fact measured over less than it declares is a different, undeclared
/// measurement.
pub(crate) async fn watch(
    mut processor: PcmProcessor,
    mut reporter: SignalMetricsReporter,
    stop: tokio_util::sync::CancellationToken,
) {
    loop {
        let frame = tokio::select! {
            biased;
            () = stop.cancelled() => break,
            frame = processor.recv() => frame,
        };
        match frame {
            Some(frame) => reporter.observe(&frame),
            None => break,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    use sipx_audio::analysis::AnalysisProfile;
    use sipx_audio::signal::SignalReporter;

    use crate::event::EventSink;
    use crate::{CallEvent, CallEvents};

    fn profile() -> SignalReportProfile {
        SignalReportProfile::new(
            AnalysisProfile::new(AudioDirection::Inbound, 8_000)
                .with_silence_timeout_ms(Some(2_000)),
        )
    }

    /// A reporter with its own event stream, standing in for one call.
    fn reporter(
        call_id: &str,
        profile: SignalReportProfile,
    ) -> (SignalMetricsReporter, EventSink, CallEvents) {
        let (sink, events) = EventSink::new();
        let reporter = SignalMetricsReporter::new(
            AudioAnalyzer::new(profile.analysis()).unwrap(),
            SignalReporter::new(profile).unwrap(),
            Arc::from(call_id),
            sink.emitter(),
        );
        (reporter, sink, events)
    }

    fn feed(reporter: &mut SignalMetricsReporter, sequence: u64, samples: &[i16]) {
        reporter.observe_samples(AudioDirection::Inbound, sequence, None, samples);
    }

    fn drained(events: &mut CallEvents) -> Vec<SignalMetrics> {
        let mut seen = Vec::new();
        while let Some(event) = events.try_recv() {
            if let CallEvent::SignalMetrics(metrics) = event {
                seen.push(metrics);
            }
        }
        seen
    }

    /// A report reaches the application with this call's identity, the direction it measured, and
    /// the coverage it was measured over.
    #[test]
    fn a_report_carries_identity_direction_and_coverage() {
        let (mut reporter, _sink, mut events) = reporter("call-a", profile());
        feed(&mut reporter, 0, &[32_767i16; 160]);

        let seen = drained(&mut events);
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert_eq!(seen[0].call_id(), "call-a");
        assert_eq!(seen[0].direction(), AudioDirection::Inbound);

        let report = seen[0].report().expect("a completed report");
        assert_eq!(report.rate, 8_000);
        assert_eq!(report.epoch, 0);
        assert_eq!(report.sequence, 0);
        assert_eq!(report.first_window, 0);
        assert_eq!(report.at_sample, 0);
        assert_eq!(report.samples, 160);
        assert_eq!(report.peak, 32_767);
        assert_eq!(report.clipping_windows, 1);
        assert_eq!(report.silent_windows, 0);
    }

    /// The cadence is what bounds a call's event rate: four windows in, one event out.
    #[test]
    fn the_cadence_bounds_how_many_events_the_audio_produces() {
        let (mut reporter, _sink, mut events) =
            reporter("call-a", profile().with_windows_per_report(4));

        for sequence in 0..4 {
            feed(&mut reporter, sequence, &[1_000i16; 160]);
        }

        let seen = drained(&mut events);
        assert_eq!(seen.len(), 1, "four windows are one report: {seen:?}");
        let report = seen[0].report().unwrap();
        assert_eq!(report.windows, 4);
        assert_eq!(report.samples, 640);
        assert_eq!(report.dc_offset_windows, 4);
    }

    /// A gap the seam failed to flag is named `Loss` rather than smoothed over, so the report that
    /// follows it belongs to a new epoch and carries none of the audio before the gap.
    #[test]
    fn an_unflagged_seam_gap_opens_a_new_epoch_instead_of_being_summed_across() {
        let (mut reporter, _sink, mut events) = reporter("call-a", profile());
        feed(&mut reporter, 0, &[1_000i16; 160]);
        drained(&mut events);

        // Seam sequence 5 with no flag: three frames vanished without the seam saying so.
        feed(&mut reporter, 5, &[0i16; 160]);

        let seen = drained(&mut events);
        assert!(
            matches!(
                seen[0].observation(),
                SignalObservation::Reset {
                    cause: ResetCause::Discontinuity {
                        kind: DiscontinuityKind::Loss
                    },
                    epoch: 1,
                }
            ),
            "expected a loss reset first: {seen:?}"
        );
        let report = seen[1].report().expect("the new epoch's first window");
        assert_eq!(report.epoch, 1);
        assert_eq!(report.first_window, 0);
        assert_eq!(report.at_sample, 0);
        assert_eq!(report.sum, 0, "none of the pre-gap audio is in it");
    }

    /// Audio the analyser refused is audio nobody measured, so the next report must not sum across
    /// it: the break is owed forward and the epoch restarts.
    ///
    /// Without that, a period straddling the refusal would name a window count and a sample span it
    /// never saw — a report claiming coverage it does not have, which is worse than no report.
    #[test]
    fn a_frame_the_analyser_refused_breaks_the_epoch_instead_of_vanishing() {
        let (mut reporter, _sink, mut events) = reporter("call-a", profile());
        feed(&mut reporter, 0, &[1_000i16; 160]);
        drained(&mut events);

        // Larger than the contract's per-frame ceiling, which the analyser refuses (§7.3).
        feed(&mut reporter, 1, &vec![1_000i16; 65_537]);
        assert!(
            drained(&mut events).is_empty(),
            "a refused frame observes nothing by itself"
        );

        feed(&mut reporter, 2, &[0i16; 160]);
        let seen = drained(&mut events);
        assert!(
            matches!(
                seen[0].observation(),
                SignalObservation::Reset {
                    cause: ResetCause::Discontinuity {
                        kind: DiscontinuityKind::Loss
                    },
                    epoch: 1,
                }
            ),
            "the unmeasured audio is owed forward as a break: {seen:?}"
        );
        let report = seen[1].report().expect("the new epoch's first window");
        assert_eq!(report.epoch, 1);
        assert_eq!(report.first_window, 0);
        assert_eq!(report.sum, 0, "none of the pre-refusal audio is in it");
    }

    /// The silence transition reaches the application, named at the run's first sample.
    #[test]
    fn the_silence_transition_reaches_the_application() {
        let (mut reporter, _sink, mut events) = reporter("call-a", profile());

        // Drained between frames, as a consumer keeping up would: a hundred 20 ms windows produce
        // a hundred reports, and a consumer that never read would lose the transition among them
        // to the stream's own overflow policy rather than to anything this module did.
        let mut elapsed = Vec::new();
        for sequence in 0..100 {
            feed(&mut reporter, sequence, &[0i16; 160]);
            elapsed.extend(drained(&mut events).into_iter().filter_map(|metrics| {
                match metrics.observation() {
                    SignalObservation::SilenceElapsed {
                        at_sample,
                        rate,
                        epoch,
                    } => Some((*at_sample, *rate, *epoch)),
                    _ => None,
                }
            }));
        }

        assert_eq!(
            elapsed,
            vec![(0, 8_000, 0)],
            "16,000 samples of silence at 8,000 Hz is the 2,000 ms timeout, named at the run's \
             first sample and in the epoch it belongs to"
        );
    }

    /// Two calls' reporters never report each other's audio: no state is shared between them.
    #[test]
    fn two_calls_never_report_each_others_samples() {
        let (mut loud, _loud_sink, mut loud_events) = reporter("call-loud", profile());
        let (mut quiet, _quiet_sink, mut quiet_events) = reporter("call-quiet", profile());

        feed(&mut loud, 0, &[32_767i16; 160]);
        feed(&mut quiet, 0, &[0i16; 160]);

        let loud_seen = drained(&mut loud_events);
        let quiet_seen = drained(&mut quiet_events);

        assert_eq!(loud_seen[0].call_id(), "call-loud");
        assert_eq!(loud_seen[0].report().unwrap().peak, 32_767);
        assert_eq!(loud_seen[0].report().unwrap().silent_windows, 0);

        assert_eq!(quiet_seen[0].call_id(), "call-quiet");
        assert_eq!(quiet_seen[0].report().unwrap().peak, 0);
        assert_eq!(quiet_seen[0].report().unwrap().silent_windows, 1);
    }

    /// A consumer that stopped reading loses history and nothing else: emission never blocks, the
    /// queue never grows, and what it lost is counted for it.
    #[test]
    fn a_consumer_that_stops_reading_loses_history_and_is_told_how_much() {
        let (mut reporter, _sink, events) = reporter("call-a", profile());

        for sequence in 0..200 {
            feed(&mut reporter, sequence, &[1_000i16; 160]);
        }

        assert!(
            events.dropped() > 0,
            "a 200-window call past the stream's capacity must count its losses"
        );
    }
}
