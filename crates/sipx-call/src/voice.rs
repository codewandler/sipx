//! Voice activity as typed call events (`M-58`).
//!
//! `sipx-audio`'s [`AudioAnalyzer`] turns PCM frames into deterministic observations
//! ([`docs/specs/call-audio-processing.md`](../../../docs/specs/call-audio-processing.md)), and
//! `sipx-media`'s one bounded seam
//! ([`docs/specs/call-audio-seam.md`](../../../docs/specs/call-audio-seam.md)) is where live call
//! audio leaves the media path. This module is the join: it attaches **through that seam**, feeds
//! the analyser, and reports its voice-start and voice-end transitions on the call's own event
//! stream as [`CallEvent::VoiceStarted`] and [`CallEvent::VoiceEnded`].
//!
//! Three properties are the reason it is a module rather than four lines in `call/mod.rs`.
//!
//! **No speech model is loaded, and none is reachable.** Voice activity here is the integer
//! variance predicate of the processing contract's §5.3 over a fixed window. Nothing in this path
//! touches a recogniser, a synthesiser or a device.
//!
//! **Delivery is bounded end to end and cannot block call media.** The seam's offer never waits
//! (its §6.2), the analyser's observation queue is a fixed ring that coalesces overflow into a
//! counted marker (§8.3), and this module's own emission is the call event stream's `try_send`.
//! A consumer that stops reading loses history, never correctness.
//!
//! **A drop may not leave activity latched.** Because only transitions are reported, a dropped
//! `VoiceStarted` followed by a delivered `VoiceEnded` would tell an application the opposite of
//! what happened. So a transition that could not be delivered is *retried against the latest
//! state* rather than queued: flapping collapses, and what an application is finally told is where
//! the call actually is. The one event that may never be lost — the terminal `VoiceEnded` that
//! closes activity when the audio finishes — travels through a slot reserved when detection
//! starts, exactly as [`CallEvent::Ended`] does.
//!
//! The same rule reaches back past delivery, into the audio itself, and that part is
//! `audio_feed`'s: a frame the analyser never measured leaves activity latched across a
//! span nobody looked at, which is the one shape of this failure delivery cannot fix. So the feed
//! owes the break forward and the next frame restarts the epoch, cutting voice at the last position
//! anyone actually measured.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use sipx_audio::analysis::{AudioAnalyzer, Observation};
use sipx_media::{PcmFrame, PcmProcessor};
use tokio::sync::watch;

use crate::audio_feed::AudioFeed;
use crate::event::{CallEvent, ReservedEmitter};

/// The vocabulary a voice-activity event is written in, re-exported from the analyser that defines
/// it (`docs/specs/call-audio-processing.md` §3.1, §5.1, §6).
///
/// Re-exported rather than restated: a second spelling of "inbound" or of "the hangover elapsed" is
/// how two layers of one stack start disagreeing about what happened.
pub use sipx_audio::analysis::{
    AnalysisError, AnalysisProfile, AudioDirection, CalibrationOutcome, CalibrationProfile,
    EffectiveThresholds, VoiceEndCause,
};

/// One voice-activity transition, placed on one call's audio timeline.
///
/// Carries everything an application needs to act on the transition without asking anything else:
/// which call it belongs to, which side of it spoke, where the transition sat in the audio, and
/// where it sits in this call's ordered stream of observations. No handle, no polling and no
/// implementation type — a host that merges several calls' event streams into one queue can still
/// tell them apart and still order them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceActivity {
    call_id: Arc<str>,
    direction: AudioDirection,
    sequence: u64,
    at_sample: u64,
    sample_rate: u32,
}

impl VoiceActivity {
    /// The `Call-ID` of the call this transition belongs to (RFC 3261 §8.1.1.4).
    #[must_use]
    pub fn call_id(&self) -> &str {
        &self.call_id
    }

    /// Which side of the call the transition was observed on.
    #[must_use]
    pub const fn direction(&self) -> AudioDirection {
        self.direction
    }

    /// This call's observation number, starting at 0.
    ///
    /// Monotonic per call and shared by both directions, so an application that receives an
    /// inbound and an outbound transition knows which happened first. It is not the seam's frame
    /// sequence and not the SIP `CSeq`.
    ///
    /// A number is spent on every transition the call *attempted* to report, so **a gap here is
    /// exactly the transitions a consumer that had fallen behind was not given** — the same reading
    /// [`CallEvents::dropped`](crate::CallEvents::dropped) counts. Delivery coalesces rather than
    /// queues, so what follows a gap is where the call is, not the next thing it did.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Where the transition sat, in samples from the start of the current analysis epoch.
    ///
    /// The epoch opens when detection starts and re-opens at every reset the analyser reports — a
    /// declared format change, or a discontinuity the seam flagged. Positions are sample counts
    /// derived from the declared rate, never a clock reading, which is what makes a recorded
    /// fixture reproduce the same positions on every machine.
    #[must_use]
    pub const fn at_sample(&self) -> u64 {
        self.at_sample
    }

    /// The rate [`Self::at_sample`] is counted at.
    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// [`Self::at_sample`] expressed as an offset into the epoch.
    ///
    /// A derived offset, not a clock read: it is exactly `at_sample / sample_rate`, and two runs of
    /// the same audio produce the same value.
    #[must_use]
    pub fn at(&self) -> Duration {
        Duration::from_nanos(
            self.at_sample
                .saturating_mul(1_000_000_000)
                .checked_div(u64::from(self.sample_rate))
                .unwrap_or(0),
        )
    }
}

/// What one call's voice-activity detection is measuring against, on the call's event stream
/// (`M-84`).
///
/// `M-60` made the same answer readable from Rust, through
/// [`Call::voice_thresholds`](crate::Call::voice_thresholds). This is the *pushed* half of it, for
/// a consumer that is not holding the `Call` — an app-protocol host, an SDK client — and it exists
/// so that being told voice started and knowing what that decision was made against are not two
/// different reachability classes.
///
/// **It carries no audio, and there is none to carry.** The processing contract's §3.3 forbids
/// retaining samples past the frame that carried them and its §8.1 enumerates the whole of an
/// analyser's state without an audio buffer in it, so every field reachable from here is a count or
/// an amplitude — see [`EffectiveThresholds`], which is the same value
/// [`Call::voice_thresholds`](crate::Call::voice_thresholds) hands back, not a second spelling of
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceThresholds {
    call_id: Arc<str>,
    at_sample: u64,
    effective: EffectiveThresholds,
}

impl VoiceThresholds {
    /// The `Call-ID` of the call these thresholds belong to (RFC 3261 §8.1.1.4).
    #[must_use]
    pub fn call_id(&self) -> &str {
        &self.call_id
    }

    /// Which side of the call's audio they are measuring.
    ///
    /// Each direction has its own analyser and its own thresholds; nothing is shared between them.
    #[must_use]
    pub const fn direction(&self) -> AudioDirection {
        self.effective.profile().direction()
    }

    /// Everything in force, as [`Call::voice_thresholds`](crate::Call::voice_thresholds) reports it
    /// (`docs/specs/call-audio-processing.md` §12.9).
    #[must_use]
    pub const fn effective(&self) -> EffectiveThresholds {
        self.effective
    }

    /// The first sample of the current epoch at which
    /// [`EffectiveThresholds::activation_amplitude`] took effect.
    ///
    /// `0` when it has been in force since the epoch opened — which is every analyser that has not
    /// calibrated, and every one whose epoch a reset has re-anchored, because §12.8 preserves the
    /// effective threshold across a reset while restarting the sample positions.
    #[must_use]
    pub const fn at_sample(&self) -> u64 {
        self.at_sample
    }

    /// The rate [`Self::at_sample`] is counted at.
    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.effective.profile().rate()
    }

    /// [`Self::at_sample`] expressed as an offset into the epoch.
    ///
    /// A derived offset, not a clock read: exactly `at_sample / sample_rate`, so two runs of the
    /// same audio produce the same value.
    #[must_use]
    pub fn at(&self) -> Duration {
        Duration::from_nanos(
            self.at_sample
                .saturating_mul(1_000_000_000)
                .checked_div(u64::from(self.sample_rate()))
                .unwrap_or(0),
        )
    }
}

/// The state a transition would put the application in, once it can be told about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Transition {
    voiced: bool,
    at_sample: u64,
    /// `None` for a start; the analyser's cause for an end.
    cause: Option<VoiceEndCause>,
}

/// Turns one seam attachment's frames into this call's voice-activity events.
///
/// Sans-I/O like the analyser it owns: it reads no clock and awaits nothing, so the whole
/// transition policy above is exercised by feeding it frames.
#[derive(Debug)]
pub(crate) struct VoiceReporter {
    feed: AudioFeed,
    call_id: Arc<str>,
    sequence: Arc<AtomicU64>,
    emitter: ReservedEmitter,
    /// Whether the application has been told voice is open.
    delivered: bool,
    /// The latest transition, delivered or not.
    latest: Option<Transition>,
    /// Where the call reads this analyser's effective thresholds from (`M-60`).
    ///
    /// A latest-value channel rather than a queue: what an application asking "what is this call
    /// measuring against?" wants is the answer now, not a history of answers. Publishing costs
    /// nothing when nothing moved, because a settled threshold is an unchanged snapshot.
    thresholds: watch::Sender<EffectiveThresholds>,
    /// The last snapshot published, so an unchanged one is not published again.
    published: EffectiveThresholds,
    /// Whether the application has ever been *told* what this call measures against (`M-84`).
    ///
    /// Set only when the announcement was actually delivered, so a full event queue postpones it
    /// rather than losing it: a consumer that had fallen behind at the first frame is still told,
    /// and what it is told is the state at the frame it was told at.
    announced: bool,
    /// Whether calibration has moved the effective threshold since the last announcement.
    moved: bool,
    /// The epoch position the activation amplitude in force took effect at.
    in_force_since: u64,
}

impl VoiceReporter {
    pub(crate) fn new(
        analyzer: AudioAnalyzer,
        call_id: Arc<str>,
        sequence: Arc<AtomicU64>,
        emitter: ReservedEmitter,
        thresholds: watch::Sender<EffectiveThresholds>,
    ) -> Self {
        let published = analyzer.thresholds();
        Self {
            feed: AudioFeed::new(analyzer),
            call_id,
            sequence,
            emitter,
            delivered: false,
            latest: None,
            thresholds,
            published,
            announced: false,
            moved: false,
            in_force_since: 0,
        }
    }

    /// Feed one frame the seam delivered.
    ///
    /// A frame the feed could not offer is not silently skipped: it owes the analyser a break,
    /// which the next accepted frame carries. Nothing is read until then, because nothing changed.
    pub(crate) fn observe(&mut self, frame: &PcmFrame) {
        if self.feed.offer(frame) {
            self.collect();
            self.announce();
            self.deliver();
            self.publish();
        }
    }

    /// Tell the application what this call is measuring against, when that is news (`M-84`).
    ///
    /// News is exactly twice: once when detection has audio for the first time — because "what is
    /// this call measuring against?" has an answer from the first frame, whether or not anything
    /// ever moves — and again whenever calibration moved the effective threshold. A settled
    /// threshold says nothing, which is §12.7's own rule carried onto the call's stream, so an
    /// analyser with no calibration profile costs exactly one event for the life of the call.
    ///
    /// It is announced **before** the frame's voice transition. The contract an app-protocol host
    /// builds on this reads a snapshot as *now* rather than as when the event happened
    /// (`docs/specs/app-contract.md` §6.3), so putting the frame of reference ahead of the decision
    /// is what lets the first `VoiceStarted` an application ever sees already say what opened it.
    ///
    /// Nothing here is retried as history: a failed announcement is re-attempted on the next frame
    /// carrying the snapshot *then*, exactly as a dropped transition is.
    fn announce(&mut self) {
        if self.announced && !self.moved {
            return;
        }
        let event = CallEvent::VoiceThresholds(VoiceThresholds {
            call_id: Arc::clone(&self.call_id),
            at_sample: self.in_force_since,
            effective: self.feed.thresholds(),
        });
        if self.emitter.try_emit(event) {
            self.announced = true;
            self.moved = false;
        }
    }

    /// Republish the analyser's thresholds, if this frame moved any of them.
    ///
    /// Reading them cannot change them — [`AudioAnalyzer::thresholds`] takes `&self` and carries no
    /// audio (the processing contract's §12.9) — so this is a copy of a handful of scalars onto a
    /// channel whose reader sees the latest value and never blocks the audio path.
    fn publish(&mut self) {
        let snapshot = self.feed.thresholds();
        if snapshot != self.published {
            self.published = snapshot;
            // discard: `send_replace` returns the value it displaced and cannot fail, and a
            // snapshot nobody is reading is simply the latest one whenever somebody starts.
            let _ = self.thresholds.send_replace(snapshot);
        }
    }

    /// Read the analyser's queue and keep only what changes the application's picture.
    fn collect(&mut self) {
        for observation in self.feed.drain() {
            match observation {
                Observation::VoiceStarted { at_sample } => {
                    self.latest = Some(Transition {
                        voiced: true,
                        at_sample,
                        cause: None,
                    });
                }
                Observation::VoiceEnded { at_sample, cause } => {
                    self.latest = Some(Transition {
                        voiced: false,
                        at_sample,
                        cause: Some(cause),
                    });
                }
                // A threshold that moved is not a transition — the call is where it was — but it
                // changes what the next one will be decided against, so it is news of its own
                // (`M-84`). Only the position is kept: the values are read off the analyser's own
                // snapshot when the announcement is built, which is what keeps a frame that spans
                // several update periods from reporting a threshold nothing is measuring against.
                Observation::ThresholdUpdated { at_sample, .. } => {
                    self.in_force_since = at_sample;
                    self.moved = true;
                }
                // §12.8 preserves the effective threshold across a reset and re-anchors the epoch,
                // so the value in force has been in force since the new epoch's first sample. That
                // is a position change and not a threshold change, and it announces nothing.
                Observation::Reset { .. } => self.in_force_since = 0,
                // Window facts, silence timeouts and the queue's loss marker are not voice-activity
                // transitions. `M-59` shapes the signal metrics out of them; this module
                // deliberately reports nothing else.
                _ => {}
            }
        }
    }

    /// Emit the latest transition, if the application is not already at that state.
    ///
    /// Nothing is queued here: a transition that cannot be delivered stays *latest* and is retried
    /// on the next observation, so an application that falls behind is told where the call is
    /// rather than where it was.
    fn deliver(&mut self) {
        let Some(latest) = self.latest else { return };
        if latest.voiced == self.delivered {
            return;
        }
        let event = self.event_for(latest);
        if self.emitter.try_emit(event) {
            self.delivered = latest.voiced;
        }
    }

    fn event_for(&self, transition: Transition) -> CallEvent {
        let activity = VoiceActivity {
            call_id: Arc::clone(&self.call_id),
            direction: self.feed.direction(),
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
            at_sample: transition.at_sample,
            sample_rate: self.feed.sample_rate(),
        };
        match transition.cause {
            None => CallEvent::VoiceStarted(activity),
            Some(cause) => CallEvent::VoiceEnded { activity, cause },
        }
    }

    /// The audio is finished: cut anything still open, through the reserved slot.
    ///
    /// The analyser's own reset is what produces the cut, so the terminal event carries the same
    /// sample position and the same [`VoiceEndCause::Cut`] a reset would report mid-call. Activity
    /// cannot survive teardown latched: either the application was never told voice opened, or it
    /// is told here that it closed.
    pub(crate) fn finish(mut self) {
        self.feed.reset();
        self.collect();
        // The reset cleared the in-progress calibration measurement and re-armed the warm-up
        // (§12.8); an application reading after teardown sees that rather than a stale period.
        self.publish();
        if !self.delivered {
            return;
        }
        // The reset above turns an open analyser into a `Cut`, which `collect` will have taken as
        // the latest transition. The filter is for the one case where it did not: an end the
        // analyser's bounded queue coalesced away (its §8.3) leaves the latest transition a *start*
        // the application has already been told about, and re-sending that would be the opposite of
        // closing activity. Its position is still the best one there is.
        let transition = self
            .latest
            .filter(|transition| !transition.voiced)
            .unwrap_or(Transition {
                voiced: false,
                at_sample: self.latest.map_or(0, |transition| transition.at_sample),
                cause: Some(VoiceEndCause::Cut),
            });
        debug_assert!(
            !transition.voiced,
            "the terminal transition closes activity"
        );
        let event = self.event_for(transition);
        self.emitter.emit_reserved(event);
    }
}

/// Drive one attachment until the call's audio is finished, or until the call stops it.
///
/// Completion is an event, not a duration: [`PcmProcessor::recv`] resolving to `None` is the seam
/// saying the session stopped or the attachment was released, so nothing here waits a fixed time to
/// learn the call is over.
///
/// `stop` is what makes the terminal cut orderable. A call's own last word is
/// [`CallEvent::Ended`], and the stream promises it is last — so the call cancels this and *joins*
/// it before ending, which puts the cut ahead of `Ended` by construction rather than by hoping two
/// tasks interleave the right way.
pub(crate) async fn watch(
    mut processor: PcmProcessor,
    mut reporter: VoiceReporter,
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
    reporter.finish();
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

    use sipx_audio::analysis::DiscontinuityKind;

    use crate::event::EventSink;
    use crate::{CallEvents, EndCause};

    fn profile() -> AnalysisProfile {
        AnalysisProfile::new(AudioDirection::Inbound, 8_000)
    }

    /// One 20 ms window of alternating full-swing modulation: the processing contract's CAP-W2
    /// pattern, which is `active` and opens voice at sample 0.
    fn modulated() -> Vec<i16> {
        (0..160)
            .map(|index| if index % 2 == 0 { 8_192 } else { -8_192 })
            .collect()
    }

    fn silence() -> Vec<i16> {
        vec![0i16; 160]
    }

    /// A reporter with its own event stream, standing in for one call.
    fn reporter(call_id: &str) -> (VoiceReporter, EventSink, CallEvents) {
        let (reporter, sink, events, _) = reporter_with(call_id, profile());
        (reporter, sink, events)
    }

    /// The same, keeping the thresholds channel a call would hold (`M-60`).
    fn reporter_with(
        call_id: &str,
        profile: AnalysisProfile,
    ) -> (
        VoiceReporter,
        EventSink,
        CallEvents,
        watch::Receiver<EffectiveThresholds>,
    ) {
        let (sink, events) = EventSink::new();
        let analyzer = AudioAnalyzer::new(profile).unwrap();
        let (publisher, thresholds) = watch::channel(analyzer.thresholds());
        let reporter = VoiceReporter::new(
            analyzer,
            Arc::from(call_id),
            Arc::new(AtomicU64::new(0)),
            sink.reserved_emitter(),
            publisher,
        );
        (reporter, sink, events, thresholds)
    }

    /// [`VoiceReporter::observe`] without a live media session: the same three steps on the same
    /// feed, entered at the samples rather than at a [`PcmFrame`].
    fn observe_samples(
        reporter: &mut VoiceReporter,
        direction: AudioDirection,
        seam_sequence: u64,
        discontinuity: Option<DiscontinuityKind>,
        samples: &[i16],
    ) {
        if reporter
            .feed
            .offer_samples(direction, seam_sequence, discontinuity, samples)
        {
            reporter.collect();
            reporter.announce();
            reporter.deliver();
            reporter.publish();
        }
    }

    fn feed(reporter: &mut VoiceReporter, sequence: u64, samples: &[i16]) {
        observe_samples(reporter, AudioDirection::Inbound, sequence, None, samples);
    }

    fn drained(events: &mut CallEvents) -> Vec<CallEvent> {
        let mut seen = Vec::new();
        while let Some(event) = events.try_recv() {
            seen.push(event);
        }
        seen
    }

    /// Everything except `M-84`'s threshold announcements.
    ///
    /// The transition tests below are about *where the call is*, and every one of them predates the
    /// announcement. Filtering it out here rather than adjusting each count keeps them saying what
    /// they were written to say; the announcement has its own tests, which assert it positively.
    fn transitions(events: &mut CallEvents) -> Vec<CallEvent> {
        drained(events)
            .into_iter()
            .filter(|event| !matches!(event, CallEvent::VoiceThresholds(_)))
            .collect()
    }

    /// Only `M-84`'s threshold announcements, in order.
    fn announcements(events: &mut CallEvents) -> Vec<VoiceThresholds> {
        drained(events)
            .into_iter()
            .filter_map(|event| match event {
                CallEvent::VoiceThresholds(thresholds) => Some(thresholds),
                _ => None,
            })
            .collect()
    }

    /// The transition an application is told about is the one the analyser found, with this call's
    /// identity, direction, ordering and sample position on it.
    #[test]
    fn a_voice_start_carries_identity_direction_sequence_and_sample_time() {
        let (mut reporter, _sink, mut events) = reporter("call-a");
        feed(&mut reporter, 0, &modulated());

        let seen = transitions(&mut events);
        assert_eq!(seen.len(), 1, "{seen:?}");
        let CallEvent::VoiceStarted(activity) = &seen[0] else {
            panic!("expected a voice start, got {seen:?}");
        };
        assert_eq!(activity.call_id(), "call-a");
        assert_eq!(activity.direction(), AudioDirection::Inbound);
        assert_eq!(activity.sequence(), 0);
        assert_eq!(activity.at_sample(), 0);
        assert_eq!(activity.sample_rate(), 8_000);
        assert_eq!(activity.at(), Duration::ZERO);
    }

    /// The hangover reaches the event stream unchanged: the end is reported at the end of the last
    /// active window, one derived hangover later.
    #[test]
    fn a_voice_end_reports_the_hangover_position_the_analyser_found() {
        let (mut reporter, _sink, mut events) = reporter("call-a");
        feed(&mut reporter, 0, &modulated());
        for sequence in 1..=10u64 {
            feed(&mut reporter, sequence, &silence());
        }

        let seen = transitions(&mut events);
        assert_eq!(seen.len(), 2, "{seen:?}");
        let CallEvent::VoiceEnded { activity, cause } = &seen[1] else {
            panic!("expected a voice end, got {seen:?}");
        };
        assert_eq!(*cause, VoiceEndCause::Hangover);
        assert_eq!(activity.at_sample(), 160);
        assert_eq!(activity.sequence(), 1, "the observation stream is ordered");
        assert_eq!(activity.at(), Duration::from_millis(20));
    }

    /// Teardown may not leave activity latched: the audio finishing cuts open voice, and the cut
    /// travels through the slot reserved for it rather than competing for capacity.
    #[test]
    fn teardown_cuts_open_voice_even_with_no_room_left() {
        let (mut reporter, sink, mut events) = reporter("call-a");
        feed(&mut reporter, 0, &modulated());

        // Fill every ordinary slot after the start has landed, so the terminal event has nowhere
        // to go except its reservation.
        for _ in 0..64 {
            sink.emit(CallEvent::Answered);
        }
        reporter.finish();

        let seen = transitions(&mut events);
        let last = seen.last().unwrap();
        let CallEvent::VoiceEnded { activity, cause } = last else {
            panic!("expected the terminal cut last, got {seen:?}");
        };
        assert_eq!(*cause, VoiceEndCause::Cut);
        assert_eq!(activity.at_sample(), 160);
    }

    /// An application that was never told voice opened is not told it closed either.
    #[test]
    fn teardown_reports_nothing_when_voice_never_opened() {
        let (mut reporter, _sink, mut events) = reporter("call-a");
        feed(&mut reporter, 0, &silence());
        reporter.finish();

        assert!(transitions(&mut events).is_empty());
    }

    /// A dropped transition is retried against the *latest* state, never replayed as history: an
    /// application that fell behind is told where the call is.
    #[test]
    fn a_dropped_transition_is_coalesced_into_the_latest_state() {
        let (mut reporter, sink, mut events) = reporter("call-a");
        // Leave no ordinary capacity, so the voice start cannot be delivered.
        for _ in 0..64 {
            sink.emit(CallEvent::Answered);
        }
        feed(&mut reporter, 0, &modulated());
        for sequence in 1..=10u64 {
            feed(&mut reporter, sequence, &silence());
        }

        // Drain everything the backlog held; the call is now inactive again, and the start that
        // never landed must not be delivered late.
        let backlog = transitions(&mut events);
        assert!(
            backlog
                .iter()
                .all(|event| matches!(event, CallEvent::Answered)),
            "{backlog:?}"
        );

        feed(&mut reporter, 11, &silence());
        assert!(
            transitions(&mut events).is_empty(),
            "the application's picture is already correct: voice is closed"
        );
    }

    /// Two simultaneous calls: identity, ordering and events are per call, and nothing crosses.
    #[test]
    fn two_simultaneous_calls_never_cross() {
        let (mut one, _sink_one, mut events_one) = reporter("call-one");
        let (mut two, _sink_two, mut events_two) = reporter("call-two");

        // Only the first call carries voice; the second carries silence for exactly as long.
        feed(&mut one, 0, &modulated());
        feed(&mut two, 0, &silence());
        for sequence in 1..=10u64 {
            feed(&mut one, sequence, &silence());
            feed(&mut two, sequence, &silence());
        }

        let seen_one = transitions(&mut events_one);
        assert_eq!(seen_one.len(), 2, "{seen_one:?}");
        for event in &seen_one {
            let activity = match event {
                CallEvent::VoiceStarted(activity) | CallEvent::VoiceEnded { activity, .. } => {
                    activity
                }
                other => panic!("unexpected event {other:?}"),
            };
            assert_eq!(activity.call_id(), "call-one");
        }
        assert!(
            transitions(&mut events_two).is_empty(),
            "the silent call observed nothing, and neither call's analyser saw the other's audio"
        );

        // Each call numbers its own observations from zero: an ordering is only meaningful within
        // one call's stream.
        feed(&mut two, 11, &modulated());
        let seen_two = transitions(&mut events_two);
        let CallEvent::VoiceStarted(activity) = &seen_two[0] else {
            panic!("expected the second call's own start, got {seen_two:?}");
        };
        assert_eq!(activity.call_id(), "call-two");
        assert_eq!(activity.sequence(), 0);
        assert_eq!(
            activity.at_sample(),
            11 * 160,
            "its own epoch, measured from its own first sample"
        );
    }

    /// A flagged discontinuity restarts the epoch and cuts open voice, without latching it.
    #[test]
    fn a_flagged_discontinuity_cuts_voice_and_reopens_the_epoch() {
        let (mut reporter, _sink, mut events) = reporter("call-a");
        feed(&mut reporter, 0, &modulated());
        assert_eq!(transitions(&mut events).len(), 1);

        observe_samples(
            &mut reporter,
            AudioDirection::Inbound,
            5,
            Some(DiscontinuityKind::Loss),
            &silence(),
        );

        let seen = transitions(&mut events);
        let CallEvent::VoiceEnded { activity, cause } = &seen[0] else {
            panic!("expected the reset to cut voice, got {seen:?}");
        };
        assert_eq!(*cause, VoiceEndCause::Cut);
        assert_eq!(activity.at_sample(), 160);
    }

    /// Audio the analyser refused is audio nobody measured, so voice may not stay latched across
    /// it: the break is owed forward, and the next accepted frame cuts open voice and reopens the
    /// epoch.
    ///
    /// Without that the analyser's state simply continues over the hole — voice open before the
    /// refusal is still open after it — so a transition that happened inside the unmeasured span is
    /// reported as nothing at all, and every position after it is measured from an origin that
    /// counts audio nobody saw. This is the qualitative half of the rule `M-59` applies
    /// quantitatively in
    /// [`a_frame_the_analyser_refused_breaks_the_epoch_instead_of_vanishing`](crate::signal_metrics).
    #[test]
    fn a_frame_the_analyser_refused_breaks_the_epoch_instead_of_vanishing() {
        let (mut reporter, _sink, mut events) = reporter("call-a");
        feed(&mut reporter, 0, &modulated());
        assert_eq!(transitions(&mut events).len(), 1, "voice opened");

        // Larger than the contract's per-frame ceiling, which the analyser refuses (§7.3). What it
        // carried is beside the point: nothing measured it.
        feed(&mut reporter, 1, &vec![8_192i16; 65_537]);
        assert!(
            transitions(&mut events).is_empty(),
            "a refused frame observes nothing by itself"
        );

        feed(&mut reporter, 2, &silence());
        feed(&mut reporter, 3, &modulated());

        let seen = transitions(&mut events);
        assert_eq!(
            seen.len(),
            2,
            "the unmeasured audio is owed forward as a break: {seen:?}"
        );
        let CallEvent::VoiceEnded { activity, cause } = &seen[0] else {
            panic!("expected the owed break to cut voice, got {seen:?}");
        };
        assert_eq!(*cause, VoiceEndCause::Cut);
        assert_eq!(
            activity.at_sample(),
            160,
            "cut at the end of the last window anyone measured"
        );
        let CallEvent::VoiceStarted(activity) = &seen[1] else {
            panic!("expected the new epoch's own start, got {seen:?}");
        };
        assert_eq!(
            activity.at_sample(),
            160,
            "the second window of the epoch the break opened, not the fourth of one spanning the \
             hole"
        );
    }

    /// A sequence gap the seam failed to flag is treated as loss rather than wedging the stream.
    #[test]
    fn an_unflagged_seam_gap_does_not_wedge_the_analyser() {
        let (mut reporter, _sink, mut events) = reporter("call-a");
        feed(&mut reporter, 0, &silence());
        feed(&mut reporter, 7, &modulated());

        let seen = transitions(&mut events);
        assert!(
            matches!(seen.first(), Some(CallEvent::VoiceStarted(_))),
            "the frame after the unflagged gap is still measured: {seen:?}"
        );
    }

    /// Two simultaneous calls calibrate independently: one call's noise never moves the other's
    /// threshold, and each reports its own (`M-60`).
    ///
    /// Calibration state is per analyser and an analyser is per call and per direction, so this is
    /// a property of the shape rather than of a lock — but the shape is what a second detection
    /// path would quietly change, and that is what this pins.
    #[test]
    fn two_simultaneous_calls_calibrate_independently() {
        let calibrated = profile().with_calibration(Some(CalibrationProfile::new()));
        let (mut quiet, _sink_one, _events_one, quiet_thresholds) =
            reporter_with("call-one", calibrated);
        let (mut noisy, _sink_two, _events_two, noisy_thresholds) =
            reporter_with("call-two", calibrated);

        // Deviation 1,000: below the 2,048 activation amplitude, so it is background rather than
        // voice, and it is exactly what the noisy call's floor should learn.
        let background: Vec<i16> = (0..160)
            .map(|index| if index % 2 == 0 { 1_000 } else { -1_000 })
            .collect();
        for sequence in 0..30u64 {
            feed(&mut quiet, sequence, &silence());
            feed(&mut noisy, sequence, &background);
        }

        assert_eq!(quiet_thresholds.borrow().activation_amplitude(), 1_408);
        assert_eq!(quiet_thresholds.borrow().observed_floor(), Some(0));
        assert_eq!(noisy_thresholds.borrow().activation_amplitude(), 1_512);
        assert_eq!(
            noisy_thresholds.borrow().observed_floor(),
            Some(1_000),
            "each call's floor is measured from its own audio"
        );
    }

    /// Reading the thresholds is a read: the events a call reports do not depend on whether anyone
    /// asked what it was measuring against (`M-60`).
    #[test]
    fn inspecting_thresholds_changes_no_event() {
        let calibrated = profile().with_calibration(Some(CalibrationProfile::new()));
        let (mut watched, _sink_one, mut watched_events, thresholds) =
            reporter_with("call-one", calibrated);
        let (mut unwatched, _sink_two, mut unwatched_events, _) =
            reporter_with("call-two", calibrated);

        for sequence in 0..30u64 {
            feed(&mut watched, sequence, &modulated());
            feed(&mut unwatched, sequence, &modulated());
            for _ in 0..8 {
                let _ = thresholds.borrow().activation_amplitude();
            }
        }

        let watched_seen = transitions(&mut watched_events);
        let unwatched_seen = transitions(&mut unwatched_events);
        assert_eq!(watched_seen.len(), unwatched_seen.len(), "{watched_seen:?}");
        assert!(
            !watched_seen.is_empty(),
            "the comparison is only worth anything if something happened"
        );
    }

    /// Teardown leaves no calibration behind: the analyser is dropped with the reporter, and the
    /// last thing an application can read is the state the terminal reset left (`M-60`).
    #[test]
    fn teardown_leaves_the_reset_calibration_state() {
        let calibrated = profile().with_calibration(Some(CalibrationProfile::new()));
        let (mut reporter, _sink, _events, thresholds) = reporter_with("call-a", calibrated);
        for sequence in 0..25u64 {
            feed(&mut reporter, sequence, &silence());
        }
        assert_eq!(
            thresholds.borrow().outcome(),
            Some(CalibrationOutcome::Applied)
        );

        reporter.finish();

        let after = *thresholds.borrow();
        assert_eq!(after.outcome(), None, "the warm-up is re-armed");
        assert_eq!(after.observed_floor(), None, "the period is discarded");
        assert_eq!(
            after.activation_amplitude(),
            1_536,
            "the threshold the call learned is what it last measured against"
        );
    }

    /// A call with detection running says what it is measuring against, whether or not it ever
    /// moves (`M-84`).
    ///
    /// The reason this is not conditional on calibration: "what was that decision made against?" has
    /// an answer from the first frame, and an application that had to infer the answer from the
    /// absence of a calibration event would be inferring it from silence.
    #[test]
    fn detection_announces_what_it_is_measuring_against_on_its_first_frame() {
        let (mut reporter, _sink, mut events) = reporter("call-a");
        feed(&mut reporter, 0, &silence());

        let seen = announcements(&mut events);
        assert_eq!(seen.len(), 1, "{seen:?}");
        let announced = &seen[0];
        assert_eq!(announced.call_id(), "call-a");
        assert_eq!(announced.direction(), AudioDirection::Inbound);
        assert_eq!(announced.sample_rate(), 8_000);
        assert_eq!(
            announced.at_sample(),
            0,
            "it has been in force since the epoch opened"
        );
        assert_eq!(announced.at(), Duration::ZERO);
        let effective = announced.effective();
        assert_eq!(effective.activation_amplitude(), 2_048, "§5.1's configured");
        assert_eq!(effective.window_samples(), 160);
        assert_eq!(effective.hangover_samples(), 1_600);
        assert_eq!(
            effective.calibration_samples(),
            None,
            "a fixed threshold says so, so nothing waits for a move that cannot come"
        );
    }

    /// A threshold that cannot move is announced once and never again (`M-84`).
    #[test]
    fn a_fixed_threshold_costs_exactly_one_announcement() {
        let (mut reporter, _sink, mut events) = reporter("call-a");
        for sequence in 0..40u64 {
            feed(&mut reporter, sequence, &modulated());
        }
        reporter.finish();

        assert_eq!(
            announcements(&mut events).len(),
            1,
            "an analyser with no calibration profile has nothing further to report"
        );
    }

    /// Every move is announced, at the sample it took effect, and a settled threshold goes quiet
    /// (`M-84`, carrying §12.7's rule onto the call's stream).
    ///
    /// The numbers are the processing contract's CAL-1: 70 all-zero frames under `K8` make twelve
    /// updates, at epoch samples 1,600 + 800·k, stepping 2,048 → 1,920 → … → 512, and then the
    /// target — `0 + margin`, clamped into the floor..=ceiling interval — is reached and the
    /// analyser goes quiet.
    #[test]
    fn every_calibration_move_is_announced_at_the_sample_it_took_effect() {
        let calibrated = profile().with_calibration(Some(CalibrationProfile::new()));
        let (mut reporter, _sink, mut events, _) = reporter_with("call-a", calibrated);
        for sequence in 0..70u64 {
            feed(&mut reporter, sequence, &silence());
        }

        let seen = announcements(&mut events);
        let moves: Vec<(u64, i32)> = seen
            .iter()
            .map(|announced| {
                (
                    announced.at_sample(),
                    announced.effective().activation_amplitude(),
                )
            })
            .collect();
        let mut expected = vec![(0u64, 2_048i32)];
        for step in 0..12i32 {
            let ordinal = u64::try_from(step).unwrap_or(0);
            expected.push((1_600 + 800 * ordinal, 1_920 - 128 * step));
        }
        assert_eq!(
            moves, expected,
            "the opening announcement, then CAL-1's twelve updates"
        );
        assert_eq!(
            seen[12].effective().calibration_samples(),
            Some(1_600),
            "and it says the threshold is one that moves"
        );

        // 512 is `0 + margin`, so the target is reached and nothing more is said.
        for sequence in 70..90u64 {
            feed(&mut reporter, sequence, &silence());
        }
        assert!(
            announcements(&mut events).is_empty(),
            "a settled threshold is silent"
        );
    }

    /// The frame of reference arrives before the decision it frames (`M-84`).
    ///
    /// An application that is told voice started and has to wait for a later event to learn what
    /// opened it is the gap this story exists to close, so the order is asserted rather than left
    /// to whichever call `observe` happens to make first.
    #[test]
    fn the_announcement_precedes_the_transition_it_frames() {
        let (mut reporter, _sink, mut events) = reporter("call-a");
        feed(&mut reporter, 0, &modulated());

        let seen = drained(&mut events);
        assert!(
            matches!(
                seen.as_slice(),
                [CallEvent::VoiceThresholds(_), CallEvent::VoiceStarted(_)]
            ),
            "{seen:?}"
        );
    }

    /// An announcement nobody had room for is retried against the state *then*, never replayed as
    /// history (`M-84`) — the same rule the transitions follow.
    #[test]
    fn an_undeliverable_announcement_is_retried_against_the_state_then() {
        let calibrated = profile().with_calibration(Some(CalibrationProfile::new()));
        let (sink, events) = EventSink::new();
        let analyzer = AudioAnalyzer::new(calibrated).unwrap();
        let (publisher, _thresholds) = watch::channel(analyzer.thresholds());
        let mut reporter = VoiceReporter::new(
            analyzer,
            Arc::from("call-a"),
            Arc::new(AtomicU64::new(0)),
            sink.reserved_emitter(),
            publisher,
        );
        let mut events = events;

        // Leave no ordinary capacity, so nothing this call reports can be delivered at all.
        for _ in 0..64 {
            sink.emit(CallEvent::Answered);
        }
        for sequence in 0..25u64 {
            feed(&mut reporter, sequence, &silence());
        }
        let backlog = drained(&mut events);
        assert!(
            backlog
                .iter()
                .all(|event| matches!(event, CallEvent::Answered)),
            "{backlog:?}"
        );

        feed(&mut reporter, 25, &silence());
        let seen = announcements(&mut events);
        assert_eq!(seen.len(), 1, "one answer, not five: {seen:?}");
        assert_eq!(
            seen[0].effective().activation_amplitude(),
            1_536,
            "where the call is, not where it was when the first frame arrived"
        );
        assert_eq!(seen[0].at_sample(), 4_000, "and when that took effect");
    }

    /// A reset re-anchors the epoch without moving the threshold, so it says nothing and the
    /// position it reports is the new epoch's (`M-84`, §12.8).
    #[test]
    fn a_reset_re_anchors_the_position_without_announcing() {
        let calibrated = profile().with_calibration(Some(CalibrationProfile::new()));
        let (mut reporter, _sink, mut events, _) = reporter_with("call-a", calibrated);
        for sequence in 0..25u64 {
            feed(&mut reporter, sequence, &silence());
        }
        assert_eq!(announcements(&mut events).len(), 5);

        observe_samples(
            &mut reporter,
            AudioDirection::Inbound,
            25,
            Some(DiscontinuityKind::Loss),
            &silence(),
        );
        assert!(
            announcements(&mut events).is_empty(),
            "the timeline broke; the threshold did not"
        );

        // The next move is measured from the new epoch's own origin, and is announced there — and
        // it continues from the threshold the previous epoch learned, because §12.8 preserves it.
        for sequence in 26..46u64 {
            feed(&mut reporter, sequence, &silence());
        }
        let seen = announcements(&mut events);
        assert_eq!(
            seen.first().map(VoiceThresholds::at_sample),
            Some(1_600),
            "the new epoch's first update, not the old epoch's position plus one"
        );
        assert_eq!(
            seen.first()
                .map(|announced| announced.effective().activation_amplitude()),
            Some(1_408),
            "one step on from the 1,536 the broken timeline left in force"
        );
    }

    /// Two calls announce their own thresholds and nothing crosses (`M-84`).
    #[test]
    fn two_simultaneous_calls_announce_their_own_thresholds() {
        let calibrated = profile().with_calibration(Some(CalibrationProfile::new()));
        let (mut quiet, _sink_one, mut events_one, _) = reporter_with("call-one", calibrated);
        let (mut noisy, _sink_two, mut events_two, _) = reporter_with("call-two", calibrated);

        let background: Vec<i16> = (0..160)
            .map(|index| if index % 2 == 0 { 1_000 } else { -1_000 })
            .collect();
        for sequence in 0..30u64 {
            feed(&mut quiet, sequence, &silence());
            feed(&mut noisy, sequence, &background);
        }

        let quiet_seen = announcements(&mut events_one);
        let noisy_seen = announcements(&mut events_two);
        assert!(quiet_seen.iter().all(|t| t.call_id() == "call-one"));
        assert!(noisy_seen.iter().all(|t| t.call_id() == "call-two"));
        assert_eq!(
            quiet_seen
                .last()
                .map(|t| t.effective().activation_amplitude()),
            Some(1_408)
        );
        assert_eq!(
            noisy_seen
                .last()
                .map(|t| t.effective().activation_amplitude()),
            Some(1_512),
            "each call announces the floor it measured from its own audio"
        );
    }

    /// The reservation costs one ordinary slot and no more, and the call's own last word still
    /// arrives.
    #[test]
    fn the_reservation_does_not_cost_the_call_its_last_word() {
        let (sink, mut events) = EventSink::new();
        let mut sink = sink;
        let reserved = sink.reserved_emitter();
        for _ in 0..64 {
            sink.emit(CallEvent::Answered);
        }
        sink.end(EndCause::LocalHangup);
        drop(reserved);

        let mut seen = Vec::new();
        while let Some(event) = events.try_recv() {
            seen.push(event);
        }
        assert!(matches!(
            seen.last(),
            Some(CallEvent::Ended(EndCause::LocalHangup))
        ));
    }
}
