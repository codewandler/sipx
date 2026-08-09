//! One-pole filters: low-pass, high-pass and a peaking band
//! (`docs/specs/call-dsp-effects.md` §8).
//!
//! Every filter here is built from the same first-order section and nothing else. That is a
//! stability decision rather than a simplicity one: a one-pole section with a coefficient in
//! `0..1` has its pole strictly inside the unit circle for **every** admissible parameter value, so
//! "stable" is a property of the shape rather than a range somebody has to keep checking. A
//! resonant second-order section would put stability at the mercy of how its coefficients round in
//! fixed point, and this story does not measure that.

use crate::analysis::AudioDirection;
use crate::dsp::contract::{
    DspCapability, DspFrame, DspResetCause, ExecutionPolicy, ExecutionProfile, FormatError,
    FrameProcessor, FrameSink, Parameter, ParameterDomain, ParameterError, ParameterSpec,
    ParameterValue, ProcessError, ResetBehavior, Scratch, StreamFormat,
};

use super::arithmetic::{narrow, scaled};
use super::frame::{Body, CHANNELS, MAX_CHANNELS, open, state_bytes, transform};

/// How far below the folding frequency a cutoff is admitted, in hundredths of the sample rate.
///
/// A cutoff at or above this is clamped to it rather than refused. The parameter domain is closed
/// independently of the rate — it has to be, because a capability is declared before a format is —
/// so a rate-dependent refusal would make one configuration valid on an 8,000 Hz call and invalid
/// on the 16,000 Hz one beside it. Clamping keeps the same configuration meaningful on both and
/// keeps the coefficient inside the shipped table.
const NYQUIST_GUARD: i64 = 45;

/// The one-pole coefficient table, in Q15.
///
/// Entry *k* is `round(32768 · g/(1 + g))` for `g = tan(π·k/128)` — the bilinear-prewarped gain of
/// a one-pole section at a cutoff of `k/128` of the sample rate. It is **shipped data and never
/// computed**: the processor contract's §4.6 forbids a transcendental function in the sample path
/// *or in coefficient derivation at run time*, because such a function is not specified to the last
/// bit and a fixture computed on one machine then stops being evidence on another. A cutoff between
/// two entries is interpolated linearly in integer arithmetic, so the coefficient a rate and a
/// cutoff produce is exactly this table and this interpolation and nothing else.
///
/// Entry 64 is the folding frequency's limit, 1.0, and is unreachable: [`NYQUIST_GUARD`] clamps
/// every cutoff to 0.45 of the rate, which lands at index 57.6. It exists so that interpolation
/// near the top has a right-hand neighbour rather than a special case.
const COEFFICIENTS: [i32; 65] = [
    0, 785, 1534, 2251, 2938, 3598, 4233, 4845, 5437, 6009, 6564, 7103, 7627, 8137, 8635, 9121,
    9598, 10064, 10522, 10971, 11414, 11850, 12280, 12705, 13125, 13541, 13954, 14363, 14770,
    15175, 15579, 15982, 16384, 16786, 17189, 17593, 17998, 18405, 18814, 19227, 19643, 20063,
    20488, 20918, 21354, 21797, 22246, 22704, 23170, 23647, 24133, 24631, 25141, 25665, 26204,
    26759, 27331, 27923, 28535, 29170, 29830, 30517, 31234, 31983, 32768,
];

/// The largest cutoff these filters admit, in hertz: the folding frequency of the highest rate the
/// linear-PCM boundary admits. Written out rather than derived from `MAX_SAMPLE_RATE` so that the
/// parameter domain stays a constant expression, and checked against it in this module's tests.
const MAX_CUTOFF_HZ: i64 = 192_000;

/// The cutoff domain, in hertz. Closed, and independent of any rate (see [`NYQUIST_GUARD`]).
const CUTOFF_DOMAIN: ParameterDomain = ParameterDomain::Integer {
    min: 1,
    max: MAX_CUTOFF_HZ,
};

const LOW_PASS_PARAMETERS: &[ParameterSpec] = &[ParameterSpec::new("cutoff_hz", CUTOFF_DOMAIN)];
const HIGH_PASS_PARAMETERS: &[ParameterSpec] = &[ParameterSpec::new("cutoff_hz", CUTOFF_DOMAIN)];
const PEAKING_PARAMETERS: &[ParameterSpec] = &[
    ParameterSpec::new("centre_hz", CUTOFF_DOMAIN),
    ParameterSpec::new("band_gain", ParameterDomain::Ratio { min: 0, max: 4_000 }),
];

/// The Q15 coefficient for a cutoff at a rate, read out of [`COEFFICIENTS`].
fn coefficient(cutoff_hz: i64, rate: u32) -> i32 {
    let rate = i64::from(rate).max(1);
    let ceiling = rate.saturating_mul(NYQUIST_GUARD) / 100;
    let cutoff = cutoff_hz.clamp(0, ceiling);
    let index = cutoff.saturating_mul(128).saturating_mul(65_536) / rate;
    let step = usize::try_from(index >> 16).unwrap_or(usize::MAX);
    let fraction = index & 0xFFFF;
    let (Some(low), Some(high)) = (
        COEFFICIENTS.get(step),
        COEFFICIENTS.get(step.saturating_add(1)),
    ) else {
        return COEFFICIENTS.last().copied().unwrap_or(0);
    };
    let span = i64::from(*high) - i64::from(*low);
    narrow(i64::from(*low) + (span.saturating_mul(fraction) >> 16))
}

/// One first-order section, and the whole of the arithmetic these three filters share.
///
/// The recurrence is the topology-preserving bilinear one-pole, carried at Q15:
///
/// ```text
/// v = (x·32768 − s)·G / 32768      rounded half away from zero
/// y = v + s
/// s = y + v
/// out = y / 32768                  rounded half away from zero
/// ```
///
/// Its transfer function is `G(1 + z⁻¹) / (1 + (2G − 1)z⁻¹)`: unity at DC for every `G`, and a pole
/// at `1 − 2G`, which is inside the unit circle for every `G` in `0..1`. At `G = 0.5` — a cutoff of
/// exactly one quarter of the rate — the pole is at the origin and the section is the two-tap
/// average `(x[n] + x[n−1])/2`, which is why every vector in the test suite runs there.
///
/// **The state is 15 bits wider than the samples, and that is load-bearing.** A section whose state
/// were kept at sample resolution would stop moving as soon as the rounded update fell below one
/// sample — a dead band of `16384/G` samples, which at a 300 Hz cutoff on a narrowband call is four
/// LSB of permanent DC offset in a filter whose job is removing DC. Carrying the state at Q15 puts
/// that dead band 32,768 times lower, where it is below the resolution of the output.
#[derive(Debug, Default, Clone, Copy)]
struct OnePole {
    /// The section's state, at Q15: the sample value times 32,768.
    state: i64,
}

impl OnePole {
    fn step(&mut self, input: i32, coefficient: i32) -> i32 {
        let difference = (i64::from(input) << 15) - self.state;
        let step = scaled(difference, i64::from(coefficient), 32_768);
        let output = self.state + step;
        self.state = output + step;
        narrow(scaled(output, 1, 32_768))
    }
}

/// A one-pole low-pass.
///
/// **What it does.** Attenuates above `cutoff_hz` at 6 dB per octave, with unity gain at DC for
/// every admissible cutoff. The coefficient comes from a shipped table — `docs/specs/call-dsp-effects.md`
/// §8.2 states which one and why it is data rather than a computation — and
/// is derived once at `prepare` and once per accepted parameter set — never per sample. A cutoff at
/// or above 0.45 of the sample rate is clamped to it rather than refused, so the same configuration
/// means the same thing on a narrowband and a wideband call.
///
/// **What it does not do.** One pole is one pole: there is no resonance, no `Q`, no order and no
/// steeper slope, and a cascade of these is not offered because two stages in a graph already are
/// one. It is not an anti-aliasing or reconstruction filter — nothing here resamples — and it is
/// not linear phase: a bilinear one-pole has the phase response it has, and this processor declares
/// `latency_positions: 0` because its group delay is not a whole-position offset that a graph could
/// sum. It also does not compensate for the bilinear transform's frequency warping beyond the
/// prewarping already built into the table.
#[derive(Debug, Clone)]
pub struct LowPass {
    body: Body,
    cutoff_hz: i64,
    coefficient: i32,
    sections: [OnePole; MAX_CHANNELS],
}

impl Default for LowPass {
    fn default() -> Self {
        Self::new()
    }
}

impl LowPass {
    /// A low-pass at 3,400 Hz: the top of the narrowband telephony band.
    #[must_use]
    pub fn new() -> Self {
        Self {
            body: Body::default(),
            cutoff_hz: 3_400,
            coefficient: 0,
            sections: [OnePole::default(); MAX_CHANNELS],
        }
    }

    fn declaration() -> DspCapability {
        DspCapability::new(super::LOW_PASS)
            .with_channels(CHANNELS)
            .with_parameters(LOW_PASS_PARAMETERS)
            .with_state_bytes(state_bytes::<Self>(0))
            .with_reset(ResetBehavior::ClearsState)
            .with_execution(ExecutionPolicy::new(ExecutionProfile::ProvenInline))
    }
}

impl FrameProcessor for LowPass {
    fn capability(&self) -> DspCapability {
        Self::declaration()
    }

    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        let capability = self.capability();
        capability.validate_parameters(parameters)?;
        for parameter in parameters {
            if let ("cutoff_hz", ParameterValue::Integer(value)) =
                (parameter.id(), parameter.value())
            {
                self.cutoff_hz = value;
            }
        }
        if let Some((_, format)) = self.body.admission().prepared() {
            self.coefficient = coefficient(self.cutoff_hz, format.sample_rate());
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
        self.coefficient = coefficient(self.cutoff_hz, format.sample_rate());
        self.sections = [OnePole::default(); MAX_CHANNELS];
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
            let sections = &mut self.sections;
            open(&mut self.body, &capability, frame, sink, &mut || {
                *sections = [OnePole::default(); MAX_CHANNELS];
            })?;
        }
        let gain = self.coefficient;
        let sections = &mut self.sections;
        transform(frame, sink, |position| {
            for (sample, section) in position.iter_mut().zip(sections.iter_mut()) {
                *sample = section.step(*sample, gain);
            }
        })
    }

    fn flush(&mut self, _sink: &mut FrameSink<'_>) -> Result<(), ProcessError> {
        self.body.admission().admit_flush()
    }

    fn reset(&mut self, _cause: DspResetCause) {
        self.body.admission().reset();
        self.sections = [OnePole::default(); MAX_CHANNELS];
    }

    fn cancel(&mut self) {
        self.body.admission().cancel();
        self.sections = [OnePole::default(); MAX_CHANNELS];
    }

    fn retained(&self) -> u32 {
        0
    }
}

/// A one-pole high-pass.
///
/// **What it does.** Subtracts a [`LowPass`]'s output from its own input, position for position, so
/// it is that filter's exact complement: `low + high = x` for every sample the arithmetic did not
/// have to clamp. It attenuates below `cutoff_hz` at 6 dB per octave and has a zero at DC, which is
/// what makes it the right tool for removing a rumble or a DC offset from a call.
///
/// **What it does not do.** Everything [`LowPass`] does not do, for the same reasons: no
/// resonance, no `Q`, no steeper slope, no linear phase, no resampling. It is also not a
/// noise gate and not a de-rumble *detector* — it removes low frequencies whether or not anything
/// down there is a problem.
#[derive(Debug, Clone)]
pub struct HighPass {
    body: Body,
    cutoff_hz: i64,
    coefficient: i32,
    sections: [OnePole; MAX_CHANNELS],
}

impl Default for HighPass {
    fn default() -> Self {
        Self::new()
    }
}

impl HighPass {
    /// A high-pass at 300 Hz: the bottom of the narrowband telephony band.
    #[must_use]
    pub fn new() -> Self {
        Self {
            body: Body::default(),
            cutoff_hz: 300,
            coefficient: 0,
            sections: [OnePole::default(); MAX_CHANNELS],
        }
    }

    fn declaration() -> DspCapability {
        DspCapability::new(super::HIGH_PASS)
            .with_channels(CHANNELS)
            .with_parameters(HIGH_PASS_PARAMETERS)
            .with_state_bytes(state_bytes::<Self>(0))
            .with_reset(ResetBehavior::ClearsState)
            .with_execution(ExecutionPolicy::new(ExecutionProfile::ProvenInline))
    }
}

impl FrameProcessor for HighPass {
    fn capability(&self) -> DspCapability {
        Self::declaration()
    }

    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        let capability = self.capability();
        capability.validate_parameters(parameters)?;
        for parameter in parameters {
            if let ("cutoff_hz", ParameterValue::Integer(value)) =
                (parameter.id(), parameter.value())
            {
                self.cutoff_hz = value;
            }
        }
        if let Some((_, format)) = self.body.admission().prepared() {
            self.coefficient = coefficient(self.cutoff_hz, format.sample_rate());
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
        self.coefficient = coefficient(self.cutoff_hz, format.sample_rate());
        self.sections = [OnePole::default(); MAX_CHANNELS];
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
            let sections = &mut self.sections;
            open(&mut self.body, &capability, frame, sink, &mut || {
                *sections = [OnePole::default(); MAX_CHANNELS];
            })?;
        }
        let gain = self.coefficient;
        let sections = &mut self.sections;
        transform(frame, sink, |position| {
            for (sample, section) in position.iter_mut().zip(sections.iter_mut()) {
                let low = section.step(*sample, gain);
                *sample = narrow(i64::from(*sample) - i64::from(low));
            }
        })
    }

    fn flush(&mut self, _sink: &mut FrameSink<'_>) -> Result<(), ProcessError> {
        self.body.admission().admit_flush()
    }

    fn reset(&mut self, _cause: DspResetCause) {
        self.body.admission().reset();
        self.sections = [OnePole::default(); MAX_CHANNELS];
    }

    fn cancel(&mut self) {
        self.body.admission().cancel();
        self.sections = [OnePole::default(); MAX_CHANNELS];
    }

    fn retained(&self) -> u32 {
        0
    }
}

/// The two sections one channel of [`Peaking`] owns.
#[derive(Debug, Default, Clone, Copy)]
struct Band {
    upper: OnePole,
    lower: OnePole,
}

/// A peaking band lift or cut.
///
/// **What it does.** Extracts a band once and scales it once:
///
/// ```text
/// band = lowpass(2·centre) − lowpass(centre/2)
/// y    = x + (band_gain − 1.0)·band
/// ```
///
/// The band is the difference of two one-pole low-passes an octave either side of `centre_hz`, so
/// it is a first-order pair with a fixed width and no resonance. `band_gain` is a ratio in
/// thousandths from 0.0 through 4.0: 1.0 is the exact identity, above it lifts the band, below it
/// cuts it, and 0.0 removes the band entirely. DC and the folding frequency are untouched at every
/// setting, because both low-passes agree there and the difference is zero — which is the arithmetic
/// statement of "a peaking filter changes a band and not a level".
///
/// **What it does not do.** `band_gain` is the scale applied to the extracted band and **not** the
/// magnitude response at the centre frequency. A first-order pair's band does not reach unity, so
/// the peak this produces is smaller than `band_gain` suggests; the parameter is named after what
/// it multiplies rather than after what a sweep would measure, and `X-109` owns the measurement
/// that would let anyone claim otherwise. There is no `Q` and no bandwidth parameter — the width is
/// two octaves and fixed — no shelving mode, and no cascade. A centre frequency whose upper edge
/// would pass 0.45 of the sample rate is clamped there, so a band pushed past the folding frequency
/// narrows and then vanishes rather than folding back.
#[derive(Debug, Clone)]
pub struct Peaking {
    body: Body,
    centre_hz: i64,
    band_gain: i32,
    upper: i32,
    lower: i32,
    bands: [Band; MAX_CHANNELS],
}

impl Default for Peaking {
    fn default() -> Self {
        Self::new()
    }
}

impl Peaking {
    /// A band at 1,000 Hz at unit gain, which is the identity until it is configured.
    #[must_use]
    pub fn new() -> Self {
        Self {
            body: Body::default(),
            centre_hz: 1_000,
            band_gain: 1_000,
            upper: 0,
            lower: 0,
            bands: [Band::default(); MAX_CHANNELS],
        }
    }

    fn declaration() -> DspCapability {
        DspCapability::new(super::PEAKING)
            .with_channels(CHANNELS)
            .with_parameters(PEAKING_PARAMETERS)
            .with_state_bytes(state_bytes::<Self>(0))
            .with_reset(ResetBehavior::ClearsState)
            .with_execution(ExecutionPolicy::new(ExecutionProfile::ProvenInline))
    }

    fn derive(&mut self, rate: u32) {
        self.upper = coefficient(self.centre_hz.saturating_mul(2), rate);
        self.lower = coefficient(self.centre_hz / 2, rate);
    }
}

impl FrameProcessor for Peaking {
    fn capability(&self) -> DspCapability {
        Self::declaration()
    }

    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        let capability = self.capability();
        capability.validate_parameters(parameters)?;
        for parameter in parameters {
            match (parameter.id(), parameter.value()) {
                ("centre_hz", ParameterValue::Integer(value)) => self.centre_hz = value,
                ("band_gain", ParameterValue::Ratio(value)) => self.band_gain = value,
                _ => {}
            }
        }
        if let Some((_, format)) = self.body.admission().prepared() {
            self.derive(format.sample_rate());
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
        self.derive(format.sample_rate());
        self.bands = [Band::default(); MAX_CHANNELS];
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
            let bands = &mut self.bands;
            open(&mut self.body, &capability, frame, sink, &mut || {
                *bands = [Band::default(); MAX_CHANNELS];
            })?;
        }
        let upper = self.upper;
        let lower = self.lower;
        let lift = i64::from(self.band_gain) - 1_000;
        let bands = &mut self.bands;
        transform(frame, sink, |position| {
            for (sample, band) in position.iter_mut().zip(bands.iter_mut()) {
                let high = band.upper.step(*sample, upper);
                let low = band.lower.step(*sample, lower);
                let extracted = i64::from(high) - i64::from(low);
                *sample = narrow(i64::from(*sample) + scaled(extracted, lift, 1_000));
            }
        })
    }

    fn flush(&mut self, _sink: &mut FrameSink<'_>) -> Result<(), ProcessError> {
        self.body.admission().admit_flush()
    }

    fn reset(&mut self, _cause: DspResetCause) {
        self.body.admission().reset();
        self.bands = [Band::default(); MAX_CHANNELS];
    }

    fn cancel(&mut self) {
        self.body.admission().cancel();
        self.bands = [Band::default(); MAX_CHANNELS];
    }

    fn retained(&self) -> u32 {
        0
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

    /// §8: the table lands exactly on 0.5 at a quarter of the rate, which is the identity every
    /// filter vector is written against.
    #[test]
    fn a_quarter_rate_cutoff_is_a_two_tap_average() {
        assert_eq!(coefficient(2_000, 8_000), 16_384);
        assert_eq!(coefficient(4_000, 16_000), 16_384);
        assert_eq!(coefficient(12_000, 48_000), 16_384);
    }

    /// §8: a cutoff past the folding frequency clamps to the guard rather than running off the
    /// end of the table, and the coefficient stays strictly inside the unit circle.
    #[test]
    fn a_cutoff_past_nyquist_clamps_to_the_guard() {
        let clamped = coefficient(1_000_000, 8_000);
        assert_eq!(clamped, coefficient(3_600, 8_000));
        assert!(clamped < 32_768, "the pole stays inside the unit circle");
        assert_eq!(coefficient(0, 8_000), 0);
        assert_eq!(coefficient(-1, 8_000), 0);
    }

    /// §8: the section has unity gain at DC, exactly rather than nearly — the Q15 state is what
    /// buys that, and a section carried at sample resolution would stop short of it.
    #[test]
    fn the_section_settles_exactly_on_dc() {
        for cutoff in [100, 300, 1_000, 3_400, 3_600] {
            let gain = coefficient(cutoff, 8_000);
            let mut section = OnePole::default();
            let mut last = 0;
            for _ in 0..8_192 {
                last = section.step(10_000, gain);
            }
            assert_eq!(last, 10_000, "cutoff {cutoff}");
        }
    }

    /// §8: the written-out cutoff ceiling is the linear-PCM boundary's folding frequency.
    #[test]
    fn the_cutoff_ceiling_follows_the_rate_domain() {
        assert_eq!(MAX_CUTOFF_HZ, i64::from(crate::pcm::MAX_SAMPLE_RATE) / 2);
    }
}
