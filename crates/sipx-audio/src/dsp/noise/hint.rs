//! The producer for the hint a reducer declares it consumes:
//! `docs/specs/call-dsp-noise-reduction.md` §10 (`M-114`).
//!
//! `M-66` shipped [`ActivityInput::Optional`] and the `voice_active` flag it names, and nothing in
//! this workspace set it: a declared input with no producer. [`ActivityHint`] is that producer's
//! **policy**, and it is a policy rather than a wire because §5.6's fifth row says a wrong hint
//! makes speech worse in both directions — so what decides when the flag is set had to be measured
//! before anything could be recommended.
//!
//! §10.5 is that measurement, and it decided two things this module would otherwise have got
//! wrong. A hint set inside the reducer's own warm-up freezes the floor §5.2 seeds from the first
//! position of the epoch, which cost the corpus's overlapping condition 320 thousandths of extra
//! speech damage; the producer therefore defers through the warm-up. And the two failure directions
//! are not symmetric the way §5.6's prose reads: a hint held past its speech costs 420 thousandths
//! and one dropped during speech costs 4, so every uncertainty here resolves toward not hinting.

use crate::analysis::{AudioDirection, Observation};
use crate::dsp::contract::{Parameter, ParameterValue};

use super::{ActivityInput, NoiseReduction};

/// The cap [`HintPolicy`] puts on one unrefreshed hold, in positions.
///
/// This is `docs/specs/call-dsp-noise-reduction.md` §5.3's `adaptation_positions` default reused
/// rather than a second number invented: 4,000 positions is how long the reducer's own floor takes
/// to recover from an under-estimate, so a hint held past one whole adaptation period has already
/// cost the estimator a complete recovery cycle. That is §5.6's fifth row arriving slowly, and the
/// cap is where it stops.
pub const DEFAULT_HOLD_POSITIONS: u32 = 4_000;

/// How long the hint outlives the observations that justify it, in positions (§10.3).
///
/// Both fields are **sample counts and never durations**, for the reason
/// `docs/specs/call-dsp-noise-reduction.md` §3.1 gives for the warm-up: sample position is the only
/// clock anything on this path has, and a wall-clock adaptation here would make `M-60`'s and
/// `M-66`'s vectors irreproducible.
///
/// Nothing is validated, exactly as in [`NoiseReduction`](super::NoiseReduction): a policy is data,
/// every value in the domain is admissible, and §10.4's degradation rule means there is no refusal
/// for this type to raise. A `hold_positions` at the top of the `u32` domain is the pinned-true
/// failure of §5.6 configured deliberately, and a `hold_positions` of 0 clears the hint at the same
/// boundary it was set at, which is the policy switched off without being removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HintPolicy {
    release_positions: u32,
    hold_positions: u32,
}

impl Default for HintPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl HintPolicy {
    /// The policy §10.3 records: no release guard, and [`DEFAULT_HOLD_POSITIONS`] of hold.
    ///
    /// The release guard defaults to **zero** because the analyser's own hangover already
    /// over-extends the trailing edge — a `VoiceEnded` is emitted a hangover after the speech it
    /// describes ended (§10.1) — and §10.5 measured that direction as the expensive one, so
    /// stacking a second guard on top of it would lengthen the failure that costs the most.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            release_positions: 0,
            hold_positions: DEFAULT_HOLD_POSITIONS,
        }
    }

    /// How many positions the hint is held past a closing transition.
    ///
    /// For a profile whose hangover is short or zero, this is where the trailing guard comes from
    /// instead.
    #[must_use]
    pub const fn with_release_positions(mut self, positions: u32) -> Self {
        self.release_positions = positions;
        self
    }

    /// The ceiling on one hold that nothing refreshes.
    #[must_use]
    pub const fn with_hold_positions(mut self, positions: u32) -> Self {
        self.hold_positions = positions;
        self
    }

    /// How many positions the hint is held past a closing transition.
    #[must_use]
    pub const fn release_positions(&self) -> u32 {
        self.release_positions
    }

    /// The ceiling on one hold that nothing refreshes.
    #[must_use]
    pub const fn hold_positions(&self) -> u32 {
        self.hold_positions
    }
}

/// Why the hint moved (§10.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HintCause {
    /// A voice transition opened, so the hint was set.
    VoiceStarted,
    /// A voice transition closed and its release guard elapsed, so the hint was cleared.
    VoiceEnded,
    /// Nothing refreshed the hold for [`HintPolicy::hold_positions`], so the hint was cleared.
    ///
    /// This is the recovery path for a closing transition that never arrived — an observation
    /// stream that simply stopped — and it is what bounds the pinned-true failure of
    /// `docs/specs/call-dsp-noise-reduction.md` §5.6.
    HoldElapsed,
    /// Observations were dropped for want of a queue slot, so the hint was cleared.
    ///
    /// One of them may have been the closing transition. §10.5 measured a hint held past its
    /// speech as the expensive failure and a hint dropped during speech as the cheap one, so an
    /// observation stream with a hole in it resolves toward not hinting.
    ObservationsLost,
    /// The epoch restarted, so there is no activity left to describe and the hint was cleared.
    EpochReset,
}

/// One movement of the hint, at the position it takes effect (§10.3).
///
/// **What it promises**: `at_position` is the position the caller passed in, which is the position
/// the parameter set takes effect at, and `lag_positions` is the exact distance between that
/// position and the sample the analyser named — so detector latency is a number a caller reads
/// rather than a property it infers.
///
/// **What it does not promise**: that the lag can be removed. It cannot. A hint about position `p`
/// cannot exist before the window containing `p` has completed, and nothing on this path has a
/// lookahead to spend on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HintChange {
    at_position: u64,
    voice_active: bool,
    lag_positions: u64,
    cause: HintCause,
}

impl HintChange {
    /// The position this change takes effect at, in the current epoch.
    #[must_use]
    pub const fn at_position(&self) -> u64 {
        self.at_position
    }

    /// The hint's new value.
    #[must_use]
    pub const fn voice_active(&self) -> bool {
        self.voice_active
    }

    /// How far this change is behind the sample the analyser named, in positions.
    ///
    /// For a leading edge that is at least the detector's window `W`, because a window's verdict
    /// does not exist until the window has completed. For a trailing edge it is at least the
    /// hangover, for the same reason applied to the other end. `0` for a cause that names no
    /// sample of its own.
    #[must_use]
    pub const fn lag_positions(&self) -> u64 {
        self.lag_positions
    }

    /// Why the hint moved.
    #[must_use]
    pub const fn cause(&self) -> HintCause {
        self.cause
    }
}

/// Turns one analyser's observations into the activity hint one reducer declares (§10).
///
/// This is the missing half of `M-66`: the reducer declares [`ActivityInput::Optional`] and honours
/// the flag, `M-58`'s analyser reports the activity, and this decides — deterministically, in
/// sample counts — when one becomes the other. It reads no clock, owns no buffer, allocates
/// nothing after construction and calls into neither side, so a media worker can drive it between
/// frames without acquiring an obligation it did not have.
///
/// # The direction of the arrow
///
/// `docs/specs/custom-call-dsp.md` §10 permits exactly one arrangement — "It may consume activity
/// as a declared parameter, never as a call" — and this type is on the *caller's* side of it. It
/// holds no reducer and no analyser: a caller feeds it drained [`Observation`]s and reads back a
/// [`Parameter`] to set. A reducer still never emits, redefines, delays or suppresses a
/// voice-activity observation, and nothing here presents a reducer as a detector.
///
/// # What it promises
///
/// - **Determinism in sample counts.** Every decision is a comparison between positions the caller
///   supplied and counts [`HintPolicy`] declares. Two producers fed one observation sequence at one
///   set of positions produce the same changes at the same positions, on every machine.
/// - **The declared parameter and no other.** [`Self::parameter`] names whatever the reducer's own
///   [`ActivityInput`] named, and returns [`None`] for [`ActivityInput::Ignored`] — §3.3's "a
///   caller wiring a detector to it is wiring it to nothing", made executable rather than read.
/// - **A bounded hold.** No sequence of observations, and no absence of them, leaves the hint set
///   for longer than [`HintPolicy::hold_positions`] past the last thing that justified it.
/// - **Degradation, never refusal.** There is no fallible method here. A hint that is absent,
///   stale or wrong leaves the reducer at `M-66`'s unhinted behaviour, which is a quality outcome
///   and not an error (§10.4).
///
/// # What it does not promise
///
/// - **That the hint is right.** It is exactly as right as the analyser that produced the
///   observations, and `docs/specs/call-audio-processing.md` §5.3's predicate is an integer
///   variance test and not a speech model. §10.5 measures what a wrong one costs in each direction.
/// - **That the hint is on time.** It is not: see [`Self::unprotected_positions`] and
///   [`Self::overheld_positions`]. The leading edge is late by at least the detector's window and
///   the trailing edge by at least its hangover; the policy's answer is to count both and to
///   resolve every uncertainty toward *not* hinting, which §10.5 prices at 4 thousandths against
///   the other direction's 420. It does not correct either edge, and it cannot.
/// - **That wiring it improves any particular call.** §10.5 measures five conditions: one improves,
///   two are unchanged and two get worse, so the wiring is opt-in and off by default. A channel
///   unlike those five is a measurement question and not a promise.
///
/// # Placement
///
/// The hint's granularity is one frame (§3.3), so a caller sets the parameter **between** frames.
/// Feed the reducer, then feed the analyser, then drain into [`Self::observe`] at the position the
/// frame ended, then [`Self::advance_to`] that same position: what the next frame is processed with
/// is then decided entirely by frames that have already been processed, which is the only placement
/// a reducer declaring zero latency can honestly be given.
#[derive(Debug, Clone)]
pub struct ActivityHint {
    direction: AudioDirection,
    activity: ActivityInput,
    warm_up_positions: u64,
    policy: HintPolicy,
    active: bool,
    voice_open: bool,
    opened_at_sample: u64,
    release_at: Option<u64>,
    expires_at: Option<u64>,
    ended_at_sample: u64,
    unprotected: u64,
    overheld: u64,
}

impl ActivityHint {
    /// A producer for one direction of one call, feeding one reducer's declaration.
    ///
    /// Everything reducer-specific is read from `reducer` rather than spelled again by the caller:
    /// the flag's name from its [`ActivityInput`], so a reducer that renames it renames this too
    /// and a reducer that consumes nothing is wired to nothing, and its warm-up from
    /// [`NoiseReduction::warm_up_positions`], because §10.2's deferral is the reducer's own
    /// declared count and not a number this policy invents.
    #[must_use]
    pub const fn new(
        direction: AudioDirection,
        reducer: NoiseReduction,
        policy: HintPolicy,
    ) -> Self {
        Self {
            direction,
            activity: reducer.activity_input(),
            warm_up_positions: reducer.warm_up_positions() as u64,
            policy,
            active: false,
            voice_open: false,
            opened_at_sample: 0,
            release_at: None,
            expires_at: None,
            ended_at_sample: 0,
            unprotected: 0,
            overheld: 0,
        }
    }

    /// The reducer's declared warm-up, which is the earliest position the hint may be set at.
    ///
    /// §5.4 holds every band gain at unity for this many positions of an epoch and says why: an
    /// epoch may open in the middle of a syllable, and "a floor seeded from speech is a floor that
    /// would attenuate speech". A hint set inside that window freezes exactly the floor the warm-up
    /// exists to distrust, so this producer does not set one there — see [`Self::observe`].
    #[must_use]
    pub const fn warm_up_positions(&self) -> u64 {
        self.warm_up_positions
    }

    /// The direction this producer follows.
    ///
    /// One analyser's, and one graph's. A processor is bound to one direction at `prepare` and an
    /// analyser at construction; carrying the same fact here is what makes wiring an inbound
    /// detector to an outbound reducer a mismatch a caller can see rather than a hint that is
    /// silently about the wrong audio.
    #[must_use]
    pub const fn direction(&self) -> AudioDirection {
        self.direction
    }

    /// The reducer's declared activity input, as it was given.
    #[must_use]
    pub const fn activity_input(&self) -> ActivityInput {
        self.activity
    }

    /// The policy in force.
    #[must_use]
    pub const fn policy(&self) -> HintPolicy {
        self.policy
    }

    /// The hint as it stands.
    #[must_use]
    pub const fn voice_active(&self) -> bool {
        self.active
    }

    /// The declared flag this producer sets, or [`None`] when the reducer consumes none.
    #[must_use]
    pub const fn parameter_id(&self) -> Option<&'static str> {
        match self.activity {
            ActivityInput::Ignored => None,
            ActivityInput::Optional { parameter } => Some(parameter),
        }
    }

    /// The parameter set to hand the reducer, or [`None`] when the reducer consumes none.
    ///
    /// Always in the reducer's declared schema, because the name came from the reducer's own
    /// declaration and [`NoiseReduction::validate`](super::NoiseReduction::validate) has already
    /// held that name to a `Flag`. A caller sets it when [`Self::voice_active`] changes; setting it
    /// every frame is `docs/specs/call-dsp-noise-reduction.md` §3.3's "the caller's choice made
    /// visible" and costs one `ParameterApplied` per frame.
    #[must_use]
    pub fn parameter(&self) -> Option<Parameter> {
        self.parameter_id()
            .map(|id| Parameter::new(id, ParameterValue::Flag(self.active)))
    }

    /// How many positions of speech began before the hint could be set, cumulatively (§10.1).
    ///
    /// The leading edge's whole cost, counted rather than described: each opening transition adds
    /// the distance between the sample the analyser named and the position its observation reached
    /// this producer. Those positions ran through the reducer unhinted, and no configuration
    /// changes that — a verdict about a window does not exist until the window has completed.
    #[must_use]
    pub const fn unprotected_positions(&self) -> u64 {
        self.unprotected
    }

    /// How many positions the hint outlived the speech it described, cumulatively (§10.1).
    ///
    /// The trailing edge's cost, and the one the policy **cannot** shorten: a closing transition
    /// does not exist until the analyser's hangover has elapsed, and this producer is downstream of
    /// that. It is also the expensive direction — §10.5 measured a hint held past its speech at 420
    /// thousandths of extra damage against 4 for one dropped during it — which is why
    /// [`HintPolicy::release_positions`] defaults to 0 and adds nothing to what is counted here.
    /// A caller watching this number climb is watching the one failure the policy has no lever for.
    #[must_use]
    pub const fn overheld_positions(&self) -> u64 {
        self.overheld
    }

    /// Feed one drained observation, delivered at `at_position` (§10.3).
    ///
    /// `at_position` is where the caller is in the current epoch — for the placement §10.2
    /// describes, the position the analysed frame ended at — and it is the position any change
    /// returned here takes effect at.
    ///
    /// The vocabulary is read narrowly and on purpose. A voice transition moves the hint; an active
    /// window refreshes a hold that is already open; an epoch reset and a hole in the observation
    /// stream both clear it. **Everything else is ignored** — an inactive window most of all,
    /// because the hangover already owns the trailing edge and second-guessing it here would be a
    /// second detector with no vectors of its own.
    ///
    /// Every uncertainty resolves the same way, toward *not* hinting, and §10.5 is why: a hint held
    /// past the speech it describes cost the corpus's overlapping condition 420 thousandths of
    /// extra speech damage, and a hint dropped during speech cost it 4. Those two numbers are what
    /// decide [`Observation::Lost`], the hold cap and the warm-up deferral below.
    ///
    /// **An opening transition inside the reducer's warm-up does not set the hint** (§10.2). It
    /// arms it: the hint is set at the first [`Self::advance_to`] at or after
    /// [`Self::warm_up_positions`], provided the analyser still has voice open there. This is the
    /// one place the policy overrides the detector, and it is the measurement of §10.5 that put it
    /// here — freezing a floor the reducer's own warm-up exists to distrust cost the corpus's
    /// overlapping-speech condition 320 thousandths of extra speech damage, which is more than the
    /// hint buys anywhere.
    pub fn observe(&mut self, at_position: u64, observation: &Observation) -> Option<HintChange> {
        match observation {
            Observation::VoiceStarted { at_sample } => {
                self.release_at = None;
                self.expires_at = Some(self.hold_from(at_position));
                if !self.voice_open {
                    self.voice_open = true;
                    self.opened_at_sample = *at_sample;
                }
                self.raise(at_position)
            }
            Observation::Window { active: true, .. } => {
                if self.voice_open && self.release_at.is_none() {
                    self.expires_at = Some(self.hold_from(at_position));
                }
                None
            }
            Observation::VoiceEnded { at_sample, .. } => {
                self.voice_open = false;
                if !self.active {
                    return None;
                }
                self.ended_at_sample = *at_sample;
                self.release_at =
                    Some(at_position.saturating_add(u64::from(self.policy.release_positions)));
                self.advance_to(at_position)
            }
            Observation::Reset { .. } => self.drop_hint(at_position, HintCause::EpochReset),
            Observation::Lost { .. } => self.drop_hint(at_position, HintCause::ObservationsLost),
            _ => None,
        }
    }

    /// Advance to a frame boundary and settle anything that came due there (§10.3).
    ///
    /// Called once per frame after the frame's observations have been fed. It is where a hint armed
    /// inside the warm-up is finally set, and where a release guard or a hold cap that came due is
    /// finally cleared. The release guard settles before the hold cap, because a transition the
    /// analyser actually reported is a better reason to clear than the absence of one.
    pub fn advance_to(&mut self, position: u64) -> Option<HintChange> {
        if !self.active {
            if self.expires_at.is_some_and(|due| position >= due) {
                // An armed hint whose hold ran out before the warm-up let it be set disarms
                // silently: no hint was in force, so there is no change to report.
                self.clear();
                return None;
            }
            return self.raise(position);
        }
        if self.release_at.is_some_and(|due| position >= due) {
            let lag = position.saturating_sub(self.ended_at_sample);
            self.overheld = self.overheld.saturating_add(lag);
            self.clear();
            return Some(HintChange {
                at_position: position,
                voice_active: false,
                lag_positions: lag,
                cause: HintCause::VoiceEnded,
            });
        }
        if self.expires_at.is_some_and(|due| position >= due) {
            self.clear();
            return Some(HintChange {
                at_position: position,
                voice_active: false,
                lag_positions: 0,
                cause: HintCause::HoldElapsed,
            });
        }
        None
    }

    /// Drop the hint and everything holding it up.
    ///
    /// For a caller whose graph restarted without the analyser saying so — a
    /// [`DspResetCause`](crate::dsp::DspResetCause) or a declared discontinuity re-opens the
    /// reducer's warm-up (§5.4), during which the reducer is the exact identity and a hint about
    /// the epoch that ended describes nothing. Positions fed afterwards belong to the new epoch and
    /// start at 0.
    pub const fn reset(&mut self) {
        self.clear();
    }

    /// Drop the hint and disarm, reporting a change only when one was in force.
    fn drop_hint(&mut self, at_position: u64, cause: HintCause) -> Option<HintChange> {
        let was_active = self.active;
        self.clear();
        was_active.then_some(HintChange {
            at_position,
            voice_active: false,
            lag_positions: 0,
            cause,
        })
    }

    /// Set the hint if the analyser has voice open and the warm-up has elapsed at `position`.
    ///
    /// The lag charged here is measured from the sample the *opening* transition named, so a hint
    /// deferred through a warm-up is charged for the whole deferral and not only for the detector's
    /// window. Those positions ran through an adapting estimator, which is what the count is for.
    fn raise(&mut self, position: u64) -> Option<HintChange> {
        if self.active || !self.voice_open || position < self.warm_up_positions {
            return None;
        }
        if self.expires_at.is_some_and(|due| position >= due) {
            return None;
        }
        self.active = true;
        let lag = position.saturating_sub(self.opened_at_sample);
        self.unprotected = self.unprotected.saturating_add(lag);
        Some(HintChange {
            at_position: position,
            voice_active: true,
            lag_positions: lag,
            cause: HintCause::VoiceStarted,
        })
    }

    /// The position an unrefreshed hold opened now would expire at.
    fn hold_from(&self, at_position: u64) -> u64 {
        at_position.saturating_add(u64::from(self.policy.hold_positions))
    }

    /// Clear the hint and every deadline, keeping only the cumulative accounting.
    const fn clear(&mut self) {
        self.active = false;
        self.voice_open = false;
        self.opened_at_sample = 0;
        self.release_at = None;
        self.expires_at = None;
        self.ended_at_sample = 0;
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
    use crate::analysis::{ResetCause, VoiceEndCause};
    use crate::dsp::contract::{DspCapability, ParameterDomain, ParameterSpec};

    const FLAG_SCHEMA: &[ParameterSpec] =
        &[ParameterSpec::new("voice_active", ParameterDomain::Flag)];

    fn optional() -> ActivityInput {
        ActivityInput::Optional {
            parameter: "voice_active",
        }
    }

    /// A reducer declaration that consumes the hint and warms up in `warm_up` positions.
    fn declaration(warm_up: u32) -> NoiseReduction {
        let declared = NoiseReduction::new(DspCapability::new("t").with_parameters(FLAG_SCHEMA))
            .with_warm_up_positions(warm_up)
            .with_activity_input(optional());
        assert_eq!(declared.validate(), Ok(()));
        declared
    }

    /// A producer for a reducer with no warm-up: the policy's own arithmetic, undeferred.
    fn producer(policy: HintPolicy) -> ActivityHint {
        ActivityHint::new(AudioDirection::Inbound, declaration(0), policy)
    }

    fn window(index: u64, active: bool) -> Observation {
        Observation::Window {
            index,
            peak: 8_192,
            sum: 0,
            energy: 0,
            clipped: 0,
            clipping: false,
            impulsive: false,
            active,
            dc_offset: false,
            silent: false,
        }
    }

    /// §10.3: the hold is refreshed by an active window and bounded when nothing refreshes it.
    #[test]
    fn an_unrefreshed_hold_is_bounded_and_a_refreshed_one_is_not() {
        let mut hint = producer(HintPolicy::new().with_hold_positions(1_000));
        hint.observe(160, &Observation::VoiceStarted { at_sample: 0 });

        for index in 1..20u64 {
            let at = 160 + index * 160;
            assert_eq!(hint.observe(at, &window(index, true)), None);
            assert_eq!(hint.advance_to(at), None);
            assert!(hint.voice_active(), "a refreshed hold does not expire");
        }

        let last = 160 + 19 * 160;
        assert_eq!(hint.advance_to(last + 999), None);
        let elapsed = hint.advance_to(last + 1_000).expect("the cap fires");
        assert_eq!(elapsed.cause(), HintCause::HoldElapsed);
        assert!(!hint.voice_active());
    }

    /// §10.3: an inactive window is not a trailing edge, and a hole in the stream is.
    #[test]
    fn an_inactive_window_holds_the_hint_and_a_hole_in_the_stream_drops_it() {
        let mut hint = producer(HintPolicy::new());
        hint.observe(160, &Observation::VoiceStarted { at_sample: 0 });

        assert_eq!(hint.observe(320, &window(1, false)), None);
        assert_eq!(
            hint.observe(480, &Observation::SilenceElapsed { at_sample: 320 }),
            None
        );
        assert!(
            hint.voice_active(),
            "the hangover owns the trailing edge, not this producer"
        );

        // A dropped observation may have been the closing transition, and §10.5 measured that
        // direction as the expensive one, so the uncertainty resolves toward not hinting.
        let lost = hint
            .observe(640, &Observation::Lost { count: 7 })
            .expect("a hole in the stream clears the hint");
        assert!(!lost.voice_active());
        assert_eq!(lost.cause(), HintCause::ObservationsLost);
        assert_eq!(hint.advance_to(800), None, "and it does not re-arm itself");

        // A producer that had no hint in force reports nothing to clear.
        let mut fresh = producer(HintPolicy::new());
        assert_eq!(fresh.observe(160, &Observation::Lost { count: 1 }), None);
    }

    /// §10.3: a transition that re-opens during a release guard cancels the release.
    #[test]
    fn a_restart_during_the_guard_cancels_the_release() {
        let mut hint = producer(HintPolicy::new().with_release_positions(320));
        hint.observe(160, &Observation::VoiceStarted { at_sample: 0 });
        assert_eq!(
            hint.observe(
                1_600,
                &Observation::VoiceEnded {
                    at_sample: 1_440,
                    cause: VoiceEndCause::Hangover,
                },
            ),
            None
        );
        assert_eq!(
            hint.observe(1_760, &Observation::VoiceStarted { at_sample: 1_600 }),
            None,
            "the hint was never cleared, so re-opening it is not a change"
        );
        assert_eq!(hint.advance_to(1_920), None, "the release was cancelled");
        assert!(hint.voice_active());
    }

    /// §10.2: a transition inside the reducer's own warm-up arms the hint and does not set it.
    #[test]
    fn a_transition_inside_the_warm_up_arms_the_hint_rather_than_setting_it() {
        let mut hint = ActivityHint::new(
            AudioDirection::Inbound,
            declaration(1_024),
            HintPolicy::new(),
        );
        assert_eq!(hint.warm_up_positions(), 1_024);

        assert_eq!(
            hint.observe(160, &Observation::VoiceStarted { at_sample: 0 }),
            None,
            "a floor seeded from speech is what the warm-up exists to distrust (§5.4)"
        );
        assert!(!hint.voice_active());
        for boundary in [320, 480, 640, 800, 960] {
            assert_eq!(hint.observe(boundary, &window(boundary / 160, true)), None);
            assert_eq!(hint.advance_to(boundary), None);
            assert!(!hint.voice_active());
        }

        let set = hint
            .advance_to(1_120)
            .expect("the first boundary past the warm-up sets it");
        assert!(set.voice_active());
        assert_eq!(set.cause(), HintCause::VoiceStarted);
        assert_eq!(
            set.lag_positions(),
            1_120,
            "the deferral is charged in full, from the sample the transition named"
        );
        assert_eq!(hint.unprotected_positions(), 1_120);

        // Voice that closed inside the warm-up never arms anything past it.
        let mut hint = ActivityHint::new(
            AudioDirection::Inbound,
            declaration(1_024),
            HintPolicy::new(),
        );
        hint.observe(160, &Observation::VoiceStarted { at_sample: 0 });
        hint.observe(
            320,
            &Observation::VoiceEnded {
                at_sample: 160,
                cause: VoiceEndCause::Hangover,
            },
        );
        assert_eq!(hint.advance_to(2_048), None);
        assert!(!hint.voice_active());
        assert_eq!(hint.unprotected_positions(), 0);
    }

    /// §10.4: a reducer that consumes nothing is wired to nothing, and the producer says so.
    #[test]
    fn an_ignored_input_produces_no_parameter_at_all() {
        let mut hint = ActivityHint::new(
            AudioDirection::Outbound,
            NoiseReduction::new(DspCapability::new("t")),
            HintPolicy::new(),
        );
        assert_eq!(hint.direction(), AudioDirection::Outbound);
        assert_eq!(hint.parameter_id(), None);
        assert_eq!(hint.parameter(), None);

        // The state machine still runs: what is absent is a parameter to set, not a decision.
        hint.observe(160, &Observation::VoiceStarted { at_sample: 0 });
        assert!(hint.voice_active());
        assert_eq!(hint.parameter(), None);
    }

    /// §10.3: an explicit reset drops the hint, and the accounting survives it.
    #[test]
    fn a_reset_drops_the_hint_and_keeps_the_accounting() {
        let mut hint = producer(HintPolicy::new());
        hint.observe(160, &Observation::VoiceStarted { at_sample: 0 });
        assert_eq!(hint.unprotected_positions(), 160);

        hint.reset();
        assert!(!hint.voice_active());
        assert_eq!(hint.unprotected_positions(), 160, "the count is cumulative");

        hint.observe(160, &Observation::VoiceStarted { at_sample: 0 });
        assert_eq!(hint.unprotected_positions(), 320);
        assert_eq!(
            hint.observe(
                320,
                &Observation::Reset {
                    cause: ResetCause::FormatChange { rate: 16_000 }
                }
            )
            .map(|change| change.cause()),
            Some(HintCause::EpochReset)
        );
    }

    /// §10.3: a closing transition seen while the hint is already down changes nothing.
    #[test]
    fn a_closing_transition_without_an_opening_one_is_not_a_change() {
        let mut hint = producer(HintPolicy::new());
        assert_eq!(
            hint.observe(
                1_600,
                &Observation::VoiceEnded {
                    at_sample: 0,
                    cause: VoiceEndCause::Cut,
                },
            ),
            None
        );
        assert_eq!(hint.overheld_positions(), 0);
        assert_eq!(hint.advance_to(3_200), None);
    }

    /// The policy is data, and every field reads back as it was written.
    #[test]
    fn the_policy_reads_back_what_it_was_given() {
        let policy = HintPolicy::default();
        assert_eq!(policy, HintPolicy::new());
        assert_eq!(policy.release_positions(), 0);
        assert_eq!(policy.hold_positions(), DEFAULT_HOLD_POSITIONS);

        let tuned = policy
            .with_release_positions(240)
            .with_hold_positions(8_000);
        assert_eq!(tuned.release_positions(), 240);
        assert_eq!(tuned.hold_positions(), 8_000);
        assert_eq!(producer(tuned).policy(), tuned);
        assert_eq!(producer(tuned).activity_input(), optional());
    }

    /// A `hold_positions` of 0 is the policy switched off, and it is switched off deterministically.
    #[test]
    fn a_zero_hold_is_the_policy_switched_off() {
        let mut hint = producer(HintPolicy::new().with_hold_positions(0));
        assert_eq!(
            hint.observe(160, &Observation::VoiceStarted { at_sample: 0 }),
            None,
            "a hold that has already expired is a hint that is never set"
        );
        assert_eq!(hint.advance_to(160), None);
        assert!(!hint.voice_active());
        assert_eq!(hint.unprotected_positions(), 0);
    }
}
