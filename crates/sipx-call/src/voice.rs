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

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use sipx_audio::PcmSamples;
use sipx_audio::analysis::{AnalysisFrame, AudioAnalyzer, DiscontinuityKind, Observation};
use sipx_media::{PcmFrame, PcmProcessor};

use crate::event::{CallEvent, ReservedEmitter};

/// The vocabulary a voice-activity event is written in, re-exported from the analyser that defines
/// it (`docs/specs/call-audio-processing.md` §3.1, §5.1, §6).
///
/// Re-exported rather than restated: a second spelling of "inbound" or of "the hangover elapsed" is
/// how two layers of one stack start disagreeing about what happened.
pub use sipx_audio::analysis::{AnalysisError, AnalysisProfile, AudioDirection, VoiceEndCause};

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
    analyzer: AudioAnalyzer,
    call_id: Arc<str>,
    sequence: Arc<AtomicU64>,
    emitter: ReservedEmitter,
    /// The frame number handed to the analyser, which counts only frames it accepted. The seam's
    /// own sequence is checked against §4 of the seam contract below and then left behind: a frame
    /// this module declines to forward must not derange the analyser's sequence expectation.
    fed: u64,
    /// The last seam sequence seen, for detecting a gap the seam failed to flag.
    seam_sequence: Option<u64>,
    /// Whether the application has been told voice is open.
    delivered: bool,
    /// The latest transition, delivered or not.
    latest: Option<Transition>,
}

impl VoiceReporter {
    pub(crate) fn new(
        analyzer: AudioAnalyzer,
        call_id: Arc<str>,
        sequence: Arc<AtomicU64>,
        emitter: ReservedEmitter,
    ) -> Self {
        Self {
            analyzer,
            call_id,
            sequence,
            emitter,
            fed: 0,
            seam_sequence: None,
            delivered: false,
            latest: None,
        }
    }

    /// Feed one frame the seam delivered.
    pub(crate) fn observe(&mut self, frame: &PcmFrame) {
        let PcmSamples::Signed16(samples) = frame.pcm().samples() else {
            // Unreachable through `Call`, which always attaches at signed 16-bit — the depth the
            // analyser's contract is written in. Ignoring rather than converting keeps this module
            // from becoming a second place where audio is reinterpreted.
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
            return;
        }

        // The seam counts every frame it offered, delivered or dropped, and always flags a gap
        // (seam §4). An unflagged gap is a defect in the seam rather than a loss to smooth over,
        // so it is named `Loss` here instead of being handed to the analyser as a legal stream.
        let mut discontinuity = discontinuity;
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
            // discard: the refused frame is dropped and the analyser's epoch is NOT broken, so a
            // voice transition spanning this frame can be missed. The seam's contract makes every
            // refusal here a defect in this join rather than in the caller's audio, which is why
            // it is a debug record and not a counter. `M-77` carries the discontinuity forward
            // instead, the way `M-59`'s reducer already does.
            tracing::debug!(%refusal, "the call audio analyser refused a seam frame");
            return;
        }
        self.fed = self.fed.saturating_add(1);

        self.collect();
        self.deliver();
    }

    /// Read the analyser's queue and keep only what changes the application's picture.
    fn collect(&mut self) {
        for observation in self.analyzer.drain() {
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
                // Window facts, silence timeouts, resets and the queue's loss marker are not
                // voice-activity transitions. `M-59` shapes the signal metrics out of them; this
                // story deliberately reports nothing else.
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
            direction: self.analyzer.direction(),
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
            at_sample: transition.at_sample,
            sample_rate: self.analyzer.sample_rate(),
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
        self.analyzer.reset();
        self.collect();
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
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

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
        let (sink, events) = EventSink::new();
        let reporter = VoiceReporter::new(
            AudioAnalyzer::new(profile()).unwrap(),
            Arc::from(call_id),
            Arc::new(AtomicU64::new(0)),
            sink.reserved_emitter(),
        );
        (reporter, sink, events)
    }

    fn feed(reporter: &mut VoiceReporter, sequence: u64, samples: &[i16]) {
        reporter.observe_samples(AudioDirection::Inbound, sequence, None, samples);
    }

    fn drained(events: &mut CallEvents) -> Vec<CallEvent> {
        let mut seen = Vec::new();
        while let Some(event) = events.try_recv() {
            seen.push(event);
        }
        seen
    }

    /// The transition an application is told about is the one the analyser found, with this call's
    /// identity, direction, ordering and sample position on it.
    #[test]
    fn a_voice_start_carries_identity_direction_sequence_and_sample_time() {
        let (mut reporter, _sink, mut events) = reporter("call-a");
        feed(&mut reporter, 0, &modulated());

        let seen = drained(&mut events);
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

        let seen = drained(&mut events);
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

        let seen = drained(&mut events);
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

        assert!(drained(&mut events).is_empty());
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
        let backlog = drained(&mut events);
        assert!(
            backlog
                .iter()
                .all(|event| matches!(event, CallEvent::Answered)),
            "{backlog:?}"
        );

        feed(&mut reporter, 11, &silence());
        assert!(
            drained(&mut events).is_empty(),
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

        let seen_one = drained(&mut events_one);
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
            drained(&mut events_two).is_empty(),
            "the silent call observed nothing, and neither call's analyser saw the other's audio"
        );

        // Each call numbers its own observations from zero: an ordering is only meaningful within
        // one call's stream.
        feed(&mut two, 11, &modulated());
        let seen_two = drained(&mut events_two);
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
        assert_eq!(drained(&mut events).len(), 1);

        reporter.observe_samples(
            AudioDirection::Inbound,
            5,
            Some(DiscontinuityKind::Loss),
            &silence(),
        );

        let seen = drained(&mut events);
        let CallEvent::VoiceEnded { activity, cause } = &seen[0] else {
            panic!("expected the reset to cut voice, got {seen:?}");
        };
        assert_eq!(*cause, VoiceEndCause::Cut);
        assert_eq!(activity.at_sample(), 160);
    }

    /// A sequence gap the seam failed to flag is treated as loss rather than wedging the stream.
    #[test]
    fn an_unflagged_seam_gap_does_not_wedge_the_analyser() {
        let (mut reporter, _sink, mut events) = reporter("call-a");
        feed(&mut reporter, 0, &silence());
        feed(&mut reporter, 7, &modulated());

        let seen = drained(&mut events);
        assert!(
            matches!(seen.first(), Some(CallEvent::VoiceStarted(_))),
            "the frame after the unflagged gap is still measured: {seen:?}"
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
