//! The shipped baseline: `docs/specs/call-dsp-noise-reduction.md` §5.

use crate::analysis::AudioDirection;
use crate::dsp::contract::{
    DspCapability, DspFrame, DspResetCause, ExecutionPolicy, ExecutionProfile, FormatError,
    FrameProcessor, FrameSink, Parameter, ParameterDomain, ParameterError, ParameterSpec,
    ParameterValue, ProcessError, RateSupport, ResetBehavior, Scratch, StreamFormat,
};
use crate::dsp::effects::{
    Body, CHANNELS, MAX_CHANNELS, OnePole, coefficient, narrow, open, scaled, state_bytes,
    transform,
};

use super::{ActivityInput, HostRequirement, NoiseReducer, NoiseReduction};

/// The rates the band edges below were shaped for, and the only ones this reducer accepts.
const RATES: &[u32] = &[8_000, 16_000, 32_000, 48_000];

/// The low/mid split, in hertz: below it is hum, rumble and the bottom of the first formant.
const LOW_EDGE_HZ: i64 = 500;

/// The mid/high split, in hertz: above it is most of the fricative energy, and most of the hiss.
const HIGH_EDGE_HZ: i64 = 2_000;

/// How many bands the split produces. Three, and each one's damage is §5.6's business.
const BANDS: usize = 3;

/// How many positions of an epoch pass before any attenuation is applied (§5.4).
///
/// An epoch may open in the middle of a syllable, and a floor seeded from speech is a floor that
/// would attenuate speech. This is how long the estimator is given to find a minimum before it is
/// allowed to act on one. It is not a fade-in and it is not a convergence guarantee.
const WARM_UP_POSITIONS: u32 = 1_024;

/// The magnitude follower's length, in positions (§5.2).
///
/// Fixed rather than declared: it is short enough that no configuration of it would be a useful
/// control, and a sixth parameter for it would be a knob whose only honest setting is this one.
const ENVELOPE_POSITIONS: i64 = 8;

/// Unity gain, in thousandths. Every gain this reducer applies is at most this: it never amplifies.
const UNITY: i32 = 1_000;

const PARAMETERS: &[ParameterSpec] = &[
    ParameterSpec::new(
        "min_band_gain",
        ParameterDomain::Ratio { min: 0, max: UNITY },
    ),
    ParameterSpec::new(
        "over_subtraction",
        ParameterDomain::Ratio {
            min: 1_000,
            max: 4_000,
        },
    ),
    ParameterSpec::new(
        "adaptation_positions",
        ParameterDomain::Integer {
            min: 1,
            max: 65_536,
        },
    ),
    ParameterSpec::new(
        "gain_slew_positions",
        ParameterDomain::Integer { min: 0, max: 4_096 },
    ),
    ParameterSpec::new("voice_active", ParameterDomain::Flag),
];

/// One band's estimate and the gain derived from it: three integers, and no sample memory at all.
#[derive(Debug, Default, Clone, Copy)]
struct Band {
    /// The magnitude follower, at Q8 — the sample magnitude times 256.
    envelope: i64,
    /// The tracked noise floor, at Q8. Descends to a new minimum in one position, creeps up
    /// otherwise, and never exceeds [`Self::envelope`] while it is adapting.
    floor: i64,
    /// The gain in force, in thousandths, slewed toward its target one position at a time.
    gain: i32,
}

impl Band {
    /// Seed both estimates from the first position of an epoch (§5.2).
    ///
    /// Unconditional, including under an activity hint: the hint governs *adaptation*, and there is
    /// nothing to adapt from before the epoch has a first position.
    const fn seed(&mut self, magnitude: i64) {
        self.envelope = magnitude;
        self.floor = magnitude;
        self.gain = UNITY;
    }

    /// Follow the magnitude, then move the floor unless a hint froze it.
    fn observe(&mut self, magnitude: i64, adaptation_positions: i64, frozen: bool) {
        let difference = magnitude - self.envelope;
        if difference != 0 {
            // A rounded update that fell below one unit would stop the follower short of its
            // target forever, so the sign carries it the rest of the way in bounded time.
            let step = difference / ENVELOPE_POSITIONS;
            self.envelope += if step == 0 { difference.signum() } else { step };
        }
        if frozen {
            return;
        }
        let creep = (self.floor / adaptation_positions).max(1);
        self.floor = self.floor.saturating_add(creep).min(self.envelope);
    }

    /// The gain this band's estimate asks for, in thousandths (§5.3).
    fn target(&self, over_subtraction: i32, min_band_gain: i32) -> i32 {
        if self.envelope <= 0 {
            return UNITY;
        }
        let subtracted = i64::from(over_subtraction).saturating_mul(self.floor) / self.envelope;
        narrow(i64::from(UNITY) - subtracted).clamp(min_band_gain, UNITY)
    }

    /// Move the gain toward `target` by at most `step` thousandths, or whole when `step` is 0.
    const fn slew(&mut self, target: i32, step: i32) {
        if step <= 0 {
            self.gain = target;
        } else if target > self.gain {
            self.gain = if self.gain + step > target {
                target
            } else {
                self.gain + step
            };
        } else if self.gain - step < target {
            self.gain = target;
        } else {
            self.gain -= step;
        }
    }
}

/// One channel's estimator: two one-pole sections and one [`Band`] per band.
#[derive(Debug, Default, Clone, Copy)]
struct Channel {
    low: OnePole,
    high: OnePole,
    bands: [Band; BANDS],
}

/// A deterministic sub-band noise suppressor (`docs/specs/call-dsp-noise-reduction.md` §5).
///
/// **What it does.** Splits each channel into three bands with two of `M-65`'s one-pole sections at
/// 500 Hz and 2,000 Hz — `b₀ = s₁`, `b₁ = s₂ − s₁`, `b₂ = x − s₂`, which sum back to `x` *exactly*
/// because the upper two are defined by subtraction. Per band it follows the magnitude, tracks a
/// noise floor that descends to any new minimum in one position and creeps up by
/// `1 / adaptation_positions` of itself otherwise, and derives a gain of
/// `1000 − over_subtraction·floor/envelope`, clamped into `min_band_gain..=1000` and slewed toward
/// its target over `gain_slew_positions`. All of it is integer arithmetic at stated widths, all of
/// its time constants are position counts, and none of it reads a clock, allocates after
/// construction or looks at anything but the samples it was handed.
///
/// **What it removes.** Additive noise that is stationary over `adaptation_positions` and quieter
/// than the speech above it, in whichever band it occupies: mains hum and rumble low down, fan,
/// road and cabin noise across the low and mid bands, hiss up top. In a band that is *only* noise
/// the gain settles at `min_band_gain` and the attenuation is exactly that ratio — 12.04 dB at the
/// default, and not one dB more.
///
/// **What it damages, whenever it is removing anything.** The gain is applied to a whole band and
/// not to the noise inside it, so everything sharing that band is attenuated with it. The low band
/// carries the first formant of a low-pitched voice, so a talker over hum loses body and sounds
/// thin. The high band carries fricatives, so /s/ and /f/ are attenuated together with the hiss
/// they physically resemble and consonants lose exactly the intelligibility a listener needs. The
/// bands are first order, so their skirts overlap widely and a cut in one is a broad tilt across
/// its neighbours. Every onset is smeared: after a pause the gain has fallen to `min_band_gain`,
/// and the returning syllable is attenuated until the slew has climbed back. Stereo channels are
/// estimated independently, so an off-centre noise source can be attenuated by different amounts on
/// the two channels and its image moves.
///
/// **When it makes speech worse than doing nothing.** Five conditions, none exotic. *Speech quieter
/// than the tracked floor* — a whisper is indistinguishable from the room and is attenuated as the
/// room. *Non-stationary noise* — a door, a keyboard, a cough are faster than the floor creeps, so
/// they pass through essentially unattenuated and then raise the floor, which attenuates the
/// sentence after them; declaring zero latency is precisely why, because nothing here can see a
/// transient coming. *Overlapping speech* — background babble that is stationary over
/// `adaptation_positions` is a floor as far as this estimator is concerned, so a room of talkers
/// pulls the floor up and the near talker down with it. *A restarting epoch* — every reset and
/// every discontinuity discards the estimate and re-opens the warm-up, so a call losing packets
/// frequently spends its time carrying unmodified noise. *A wrong `voice_active` hint* — pinned
/// true the floor freezes on whatever it had, pinned false through speech the floor climbs into the
/// speech and the speech is then subtracted from itself.
///
/// And one setting is worse than all five: at `adaptation_positions = 1` the floor reaches the
/// envelope within a handful of positions, at which point every signal is its own noise floor,
/// every band sits at `min_band_gain` and nothing recovers from anything. That is a property of the
/// recurrence rather than a defect — the sample-exact vectors use it to reach a settled state in a
/// few positions — and it is not a setting for a call.
///
/// **What it never claims.** Not intelligibility, not "clean", and no dB figure other than the one
/// `min_band_gain` bounds it to. It is not a voice activity detector, not an echo canceller, not a
/// recogniser and not an automatic level: every gain it applies is at most unity, so it cannot make
/// a quiet talker louder. It loads no model, opens no socket and reads no file, because the
/// processor contract gives it none of the three.
#[derive(Debug, Clone)]
pub struct SubbandSuppressor {
    body: Body,
    channels: [Channel; MAX_CHANNELS],
    low_coefficient: i32,
    high_coefficient: i32,
    min_band_gain: i32,
    over_subtraction: i32,
    adaptation_positions: i64,
    gain_slew_positions: i32,
    voice_active: bool,
    /// Positions consumed since the epoch opened, saturating once the warm-up is behind it.
    consumed: u32,
    /// Whether the first position of the current epoch has seeded the estimates.
    primed: bool,
}

impl Default for SubbandSuppressor {
    fn default() -> Self {
        Self::new()
    }
}

impl SubbandSuppressor {
    /// A suppressor at §5.3's defaults: a floor of 0.25, 1.5× over-subtraction, a 4,000-position
    /// adaptation, a 64-position gain slew and no activity hint.
    #[must_use]
    pub fn new() -> Self {
        Self {
            body: Body::default(),
            channels: [Channel::default(); MAX_CHANNELS],
            low_coefficient: 0,
            high_coefficient: 0,
            min_band_gain: 250,
            over_subtraction: 1_500,
            adaptation_positions: 4_000,
            gain_slew_positions: 64,
            voice_active: false,
            consumed: 0,
            primed: false,
        }
    }

    fn declaration() -> DspCapability {
        DspCapability::new(super::SUBBAND_SUPPRESSOR)
            .with_rates(RateSupport::Exactly(RATES))
            .with_channels(CHANNELS)
            .with_parameters(PARAMETERS)
            .with_state_bytes(state_bytes::<Self>(0))
            .with_reset(ResetBehavior::ClearsState)
            .with_execution(ExecutionPolicy::new(ExecutionProfile::ProvenInline))
    }

    /// Discard the estimate and re-open the warm-up. The declared parameters survive (§8.1).
    fn clear(channels: &mut [Channel; MAX_CHANNELS], consumed: &mut u32, primed: &mut bool) {
        *channels = [Channel::default(); MAX_CHANNELS];
        *consumed = 0;
        *primed = false;
    }

    /// The gain slew's per-position step, in thousandths. Zero means "apply the target whole".
    const fn slew_step(&self) -> i32 {
        if self.gain_slew_positions <= 0 {
            return 0;
        }
        (UNITY + self.gain_slew_positions - 1) / self.gain_slew_positions
    }
}

impl FrameProcessor for SubbandSuppressor {
    fn capability(&self) -> DspCapability {
        Self::declaration()
    }

    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        let capability = self.capability();
        capability.validate_parameters(parameters)?;
        for parameter in parameters {
            match (parameter.id(), parameter.value()) {
                ("min_band_gain", ParameterValue::Ratio(value)) => self.min_band_gain = value,
                ("over_subtraction", ParameterValue::Ratio(value)) => self.over_subtraction = value,
                ("adaptation_positions", ParameterValue::Integer(value)) => {
                    self.adaptation_positions = value.max(1);
                }
                ("gain_slew_positions", ParameterValue::Integer(value)) => {
                    self.gain_slew_positions = i32::try_from(value).unwrap_or(0);
                }
                ("voice_active", ParameterValue::Flag(value)) => self.voice_active = value,
                _ => {}
            }
        }
        self.body.mark(&capability, parameters);
        Ok(())
    }

    fn prepare(
        &mut self,
        direction: AudioDirection,
        format: StreamFormat,
    ) -> Result<(), FormatError> {
        let capability = self.capability();
        self.body
            .admission()
            .prepare(&capability, direction, format)?;
        // Both coefficients come from `M-65`'s shipped table, derived once here and never per
        // sample: §4.6 forbids a transcendental in coefficient derivation at run time.
        self.low_coefficient = coefficient(LOW_EDGE_HZ, format.sample_rate());
        self.high_coefficient = coefficient(HIGH_EDGE_HZ, format.sample_rate());
        Self::clear(&mut self.channels, &mut self.consumed, &mut self.primed);
        Ok(())
    }

    fn process(
        &mut self,
        frame: &DspFrame<'_>,
        _scratch: &mut Scratch<'_>,
        sink: &mut FrameSink<'_>,
    ) -> Result<(), ProcessError> {
        let capability = self.capability();
        {
            let channels = &mut self.channels;
            let consumed = &mut self.consumed;
            let primed = &mut self.primed;
            open(&mut self.body, &capability, frame, sink, &mut || {
                Self::clear(channels, consumed, primed);
            })?;
        }

        let low_coefficient = self.low_coefficient;
        let high_coefficient = self.high_coefficient;
        let min_band_gain = self.min_band_gain;
        let over_subtraction = self.over_subtraction;
        let adaptation_positions = self.adaptation_positions;
        let slew_step = self.slew_step();
        let voice_active = self.voice_active;
        let channels = &mut self.channels;
        let mut consumed = self.consumed;
        let mut primed = self.primed;

        let outcome = transform(frame, sink, |position| {
            let warming = consumed < WARM_UP_POSITIONS;
            for (sample, channel) in position.iter_mut().zip(channels.iter_mut()) {
                let input = *sample;
                let low = channel.low.step(input, low_coefficient);
                let high = channel.high.step(input, high_coefficient);
                let bands = [low, high - low, input - high];

                let mut sum: i64 = 0;
                for (band, value) in channel.bands.iter_mut().zip(bands) {
                    let magnitude = i64::from(value).abs() * 256;
                    if primed {
                        band.observe(magnitude, adaptation_positions, voice_active);
                        if warming {
                            band.gain = UNITY;
                        } else {
                            let target = band.target(over_subtraction, min_band_gain);
                            band.slew(target, slew_step);
                        }
                    } else {
                        band.seed(magnitude);
                    }
                    sum += scaled(i64::from(value), i64::from(band.gain), i64::from(UNITY));
                }
                *sample = narrow(sum);
            }
            primed = true;
            consumed = consumed.saturating_add(1).min(WARM_UP_POSITIONS);
        });

        self.consumed = consumed;
        self.primed = primed;
        outcome
    }

    fn flush(&mut self, _sink: &mut FrameSink<'_>) -> Result<(), ProcessError> {
        self.body.admission().admit_flush()
    }

    fn reset(&mut self, _cause: DspResetCause) {
        self.body.admission().reset();
        Self::clear(&mut self.channels, &mut self.consumed, &mut self.primed);
    }

    fn cancel(&mut self) {
        self.body.admission().cancel();
        Self::clear(&mut self.channels, &mut self.consumed, &mut self.primed);
    }

    fn retained(&self) -> u32 {
        // Zero at every point in the lifecycle, and there is nothing for it to be otherwise:
        // latency and tail are 0, the frame's samples are borrowed and never copied into state,
        // and what the estimator keeps is a magnitude rather than a sample.
        0
    }
}

impl NoiseReducer for SubbandSuppressor {
    fn noise_reduction(&self) -> NoiseReduction {
        NoiseReduction::new(Self::declaration())
            .with_warm_up_positions(WARM_UP_POSITIONS)
            .with_activity_input(ActivityInput::Optional {
                parameter: "voice_active",
            })
            .with_host_requirement(HostRequirement::PortableInteger)
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

    /// §5: the shipped declaration is admissible, and it is the one the module's list names.
    #[test]
    fn the_shipped_declaration_is_admissible() {
        let reducer = SubbandSuppressor::new();
        reducer.noise_reduction().validate().unwrap();
        assert_eq!(reducer.capability().id(), super::super::SUBBAND_SUPPRESSOR);
        assert_eq!(
            reducer.noise_reduction().warm_up_positions(),
            WARM_UP_POSITIONS
        );
    }

    /// §5.3: the slew step is the ceiling of a full swing over the declared length, and 0 means
    /// the target is taken whole.
    #[test]
    fn the_slew_step_covers_a_full_swing_in_the_declared_positions() {
        let mut reducer = SubbandSuppressor::new();
        assert_eq!(reducer.slew_step(), 16, "ceil(1000 / 64)");
        reducer.gain_slew_positions = 0;
        assert_eq!(reducer.slew_step(), 0, "a whole change");
        reducer.gain_slew_positions = 4_096;
        assert_eq!(reducer.slew_step(), 1, "the slowest slew the domain admits");
    }

    /// §5.3: a slew lands exactly on its target rather than stepping past it.
    #[test]
    fn a_slew_lands_on_its_target() {
        let mut band = Band {
            envelope: 0,
            floor: 0,
            gain: UNITY,
        };
        for _ in 0..46 {
            band.slew(250, 16);
        }
        assert_eq!(band.gain, 264);
        band.slew(250, 16);
        assert_eq!(band.gain, 250, "the last step lands, it does not overshoot");
        band.slew(250, 16);
        assert_eq!(band.gain, 250);

        band.slew(1_000, 16);
        assert_eq!(band.gain, 266, "and it climbs the same way");
    }

    /// §5.2: the floor never exceeds the envelope while it is adapting, and freezes when told to.
    #[test]
    fn the_floor_is_capped_by_the_envelope_unless_it_is_frozen() {
        let mut band = Band::default();
        band.seed(1_000);
        assert_eq!((band.envelope, band.floor), (1_000, 1_000));

        // A rising envelope: the floor creeps after it and never past it.
        for _ in 0..64 {
            band.observe(100_000, 4_000, false);
            assert!(band.floor <= band.envelope, "the floor is a floor");
        }

        // A new minimum below the floor arrives in one position, which is the whole of what
        // "recursive minimum with an upward creep" means.
        let mut settled = Band {
            envelope: 100_000,
            floor: 100_000,
            gain: UNITY,
        };
        settled.observe(0, 4_000, false);
        assert_eq!(
            settled.envelope, 87_500,
            "the follower moves an eighth of the way"
        );
        assert_eq!(settled.floor, 87_500, "the floor descends to it at once");

        // Frozen, it does not move at all — up or down.
        let held = settled.floor;
        for _ in 0..64 {
            settled.observe(100_000, 4_000, true);
        }
        assert_eq!(settled.floor, held);
        settled.observe(0, 4_000, true);
        assert_eq!(settled.floor, held);
    }

    /// §5.3: the guard on an empty band, and the clamp on a frozen floor above its envelope.
    #[test]
    fn a_gain_target_is_bounded_at_both_ends() {
        let empty = Band::default();
        assert_eq!(
            empty.target(1_500, 250),
            UNITY,
            "silence is never attenuated"
        );

        let stationary = Band {
            envelope: 1_000,
            floor: 1_000,
            gain: UNITY,
        };
        assert_eq!(stationary.target(1_500, 250), 250);
        assert_eq!(
            stationary.target(1_000, 250),
            250,
            "1000 − 1000 = 0, clamped"
        );

        let half = Band {
            envelope: 1_000,
            floor: 500,
            gain: UNITY,
        };
        assert_eq!(half.target(1_000, 250), 500);
        assert_eq!(half.target(1_500, 250), 250);

        let stale = Band {
            envelope: 10,
            floor: 10_000,
            gain: UNITY,
        };
        assert_eq!(
            stale.target(1_500, 250),
            250,
            "a frozen floor above its band"
        );
    }
}
