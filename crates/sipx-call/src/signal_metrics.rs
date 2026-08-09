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

use sipx_audio::analysis::AudioAnalyzer;
use sipx_media::{PcmFrame, PcmProcessor};

use crate::audio_feed::AudioFeed;
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
    /// One observation, placed on a call's timeline from its parts (`M-98`).
    ///
    /// Everything a call reports goes through here; this is the same construction, reachable from
    /// outside the crate. Nothing outside could build one at all, which is why
    /// `sipx-app-protocol`'s bridge into the application contract had no tests until `M-98` and
    /// shipped for two releases with no arm for this event at all — `call.signal.metrics` and
    /// `call.signal.silence` were specified, typed, round-tripped, and unreachable.
    ///
    /// It states, it does not measure. `observation` is carried through verbatim, so a report
    /// handed in here claims whatever coverage it was built with — see [`measure`] for the
    /// constructor that runs the real reducer over real samples, which is the one to reach for
    /// when the numbers themselves are the thing under test.
    #[must_use]
    pub fn new(call_id: &str, direction: AudioDirection, observation: SignalObservation) -> Self {
        Self {
            call_id: Arc::from(call_id),
            direction,
            observation,
        }
    }

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

/// Measure one contiguous run of samples exactly as a call with
/// [`Call::report_signal_metrics`](crate::Call::report_signal_metrics) running would (`M-98`).
///
/// The same analyser, the same reducer, the same cadence and the same feed that keeps a live
/// attachment honest. Only the seam is absent, and with it the media session, the runtime and the
/// call. The samples are offered a window at a time, contiguously and with no discontinuity —
/// which is what a call's audio arriving on time looks like — and the direction and rate are the
/// profile's.
///
/// Every observation the reducer completed comes back in the order it completed, as the values the
/// call's [`CallEvent::SignalMetrics`] events would have carried. Nothing is flushed at the end,
/// for the same reason a live attachment flushes nothing when the call's audio finishes: a
/// reporting period cut short covers fewer windows than it claims, and a fact measured over less
/// than it declares is a different measurement.
///
/// This is what makes a *consumer* of these events testable against numbers nobody typed.
/// `sipx-app-protocol`'s bridge into the application contract is the caller it was added for
/// (`M-98`), which had no tests at all while there was no way to build its input. It reads no clock
/// and awaits nothing, so the same samples give the same reports on every machine.
///
/// # Errors
///
/// [`SignalProfileError`] for a profile outside the processing contract's domains or a cadence
/// outside `1..=`[`MAX_WINDOWS_PER_REPORT`]. Refused before anything is measured.
pub fn measure(
    call_id: &str,
    profile: SignalReportProfile,
    samples: &[i16],
) -> Result<Vec<SignalMetrics>, SignalProfileError> {
    // Both are validated before a sample is looked at, so a refused configuration measures nothing.
    let mut reducer = sipx_audio::signal::SignalReporter::new(profile)?;
    let mut feed = AudioFeed::new(AudioAnalyzer::new(profile.analysis())?);

    let call_id: Arc<str> = Arc::from(call_id);
    let direction = profile.analysis().direction();
    let frame_samples = usize::try_from(feed.window_samples())
        .unwrap_or(usize::MAX)
        .max(1);

    let mut produced = Vec::new();
    let mut sequence = 0u64;
    for frame in samples.chunks(frame_samples) {
        let offered = feed.offer_samples(direction, sequence, None, frame);
        sequence = sequence.saturating_add(1);
        if !offered {
            continue;
        }
        // Read after the frame was processed, exactly as `SignalMetricsReporter::report` does.
        let window = feed.window_samples();
        for observation in feed.drain() {
            if let Some(signal) = reducer.observe(&observation, window) {
                produced.push(SignalMetrics {
                    call_id: Arc::clone(&call_id),
                    direction,
                    observation: signal,
                });
            }
        }
    }
    Ok(produced)
}

/// Turns one seam attachment's frames into this call's signal-metric events.
///
/// Sans-I/O like the analyser and the reducer it owns: it reads no clock and awaits nothing, so
/// the whole reporting policy is exercised by feeding it frames.
#[derive(Debug)]
pub(crate) struct SignalMetricsReporter {
    feed: AudioFeed,
    reporter: sipx_audio::signal::SignalReporter,
    call_id: Arc<str>,
    emitter: Emitter,
}

impl SignalMetricsReporter {
    pub(crate) fn new(
        analyzer: AudioAnalyzer,
        reporter: sipx_audio::signal::SignalReporter,
        call_id: Arc<str>,
        emitter: Emitter,
    ) -> Self {
        Self {
            feed: AudioFeed::new(analyzer),
            reporter,
            call_id,
            emitter,
        }
    }

    /// Feed one frame the seam delivered.
    ///
    /// A frame the feed could not offer is not silently skipped: it owes the analyser a break,
    /// which the next accepted frame carries, so no report can sum across the hole and claim
    /// coverage of samples that never reached the analyser. Nothing is reduced until then, because
    /// nothing changed.
    pub(crate) fn observe(&mut self, frame: &PcmFrame) {
        if self.feed.offer(frame) {
            self.report(frame.direction());
        }
    }

    /// Reduce whatever the analyser produced and emit what the reducer completed.
    ///
    /// The window length is read from the analyser *after* it processed the frame, so a format
    /// change would be reflected in the same call that reported it. Through `Call` no format
    /// change is ever declared — a media-session replacement re-attaches instead (`M-58`'s
    /// pattern), and an in-session clock change arrives as a `Realign` — so in practice this is
    /// constant for the attachment's life.
    fn report(&mut self, direction: AudioDirection) {
        let window = self.feed.window_samples();
        let mut produced: Vec<SignalObservation> = Vec::new();
        for observation in self.feed.drain() {
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

    use sipx_audio::analysis::{AnalysisProfile, DiscontinuityKind};
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

    /// [`SignalMetricsReporter::observe`] without a live media session: the same two steps on the
    /// same feed, entered at the samples rather than at a [`PcmFrame`].
    fn observe_samples(
        reporter: &mut SignalMetricsReporter,
        direction: AudioDirection,
        seam_sequence: u64,
        discontinuity: Option<DiscontinuityKind>,
        samples: &[i16],
    ) {
        if reporter
            .feed
            .offer_samples(direction, seam_sequence, discontinuity, samples)
        {
            reporter.report(direction);
        }
    }

    fn feed(reporter: &mut SignalMetricsReporter, sequence: u64, samples: &[i16]) {
        observe_samples(reporter, AudioDirection::Inbound, sequence, None, samples);
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

    /// [`measure`] is this module's reporter with the call taken away, and its promise is that the
    /// observations are the same ones (`M-98`).
    ///
    /// Worth a test rather than a sentence, for the reason the story that added it exists: a
    /// consumer of these events — `sipx-app-protocol`'s bridge into the application contract — had
    /// no tests at all while nothing outside this crate could build the input, and a fixture that
    /// measured differently from a live call would make every such test agree with the wrong thing.
    #[test]
    fn measuring_samples_gives_what_a_running_call_would_have_reported() {
        // Four windows: modulated, silent, clipping, and modulated again, so the run covers a
        // report that is active, one that is silent and one that clips rather than four alike.
        let mut samples = Vec::new();
        samples.extend((0..160).map(|index| if index % 2 == 0 { 8_192i16 } else { -8_192 }));
        samples.extend([0i16; 160]);
        samples.extend([32_767i16; 160]);
        samples.extend((0..160).map(|index| if index % 2 == 0 { 8_192i16 } else { -8_192 }));

        let (mut live, _sink, mut events) = reporter("call-a", profile());
        for (sequence, frame) in samples.chunks(160).enumerate() {
            feed(&mut live, u64::try_from(sequence).unwrap(), frame);
        }
        let reported = drained(&mut events);
        assert_eq!(reported.len(), 4, "one report per window: {reported:?}");

        assert_eq!(
            measure("call-a", profile(), &samples).unwrap(),
            reported,
            "the same samples, the same analyser and the same reducer"
        );
    }

    /// [`SignalMetrics::new`] carries every part through and invents none.
    #[test]
    fn a_stated_observation_carries_exactly_what_it_was_given() {
        let observation = SignalObservation::SilenceElapsed {
            at_sample: 320,
            rate: 8_000,
            epoch: 2,
        };
        let metrics = SignalMetrics::new("call-a", AudioDirection::Outbound, observation);
        assert_eq!(metrics.call_id(), "call-a");
        assert_eq!(metrics.direction(), AudioDirection::Outbound);
        assert_eq!(*metrics.observation(), observation);
        assert!(
            metrics.report().is_none(),
            "a silence transition is not a completed report"
        );
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
