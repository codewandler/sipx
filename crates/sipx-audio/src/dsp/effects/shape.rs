//! Waveshaping: hard clipping, soft clipping and bit crushing
//! (`docs/specs/call-dsp-effects.md` §6).

use crate::analysis::AudioDirection;
use crate::dsp::contract::{
    DspCapability, DspFrame, DspResetCause, ExecutionPolicy, ExecutionProfile, FormatError,
    FrameProcessor, FrameSink, Parameter, ParameterDomain, ParameterError, ParameterSpec,
    ParameterValue, ProcessError, ResetBehavior, Scratch, StreamFormat,
};

use super::arithmetic::narrow;
use super::frame::{Body, CHANNELS, MAX_CHANNELS, open, state_bytes, transform};

/// Full scale, as the positive magnitude both clippers are written against.
const FULL_SCALE: i64 = 32_767;

/// The longest sample-and-hold this processor admits, in positions.
const MAX_HOLD: i64 = 256;

const HARD_CLIP_PARAMETERS: &[ParameterSpec] = &[ParameterSpec::new(
    "ceiling",
    ParameterDomain::Integer {
        min: 0,
        max: FULL_SCALE,
    },
)];

/// A symmetric hard clipper.
///
/// **What it does.** Limits every sample to `-ceiling..=ceiling`, a closed integer range from 0
/// through 32,767. The transfer function is linear below the ceiling and flat at it, which is the
/// definition of hard clipping and the reason it generates odd harmonics.
///
/// The clip is symmetric, so at the default ceiling `i16::MIN` becomes `-32_767`: the one sample
/// the representable range is asymmetric about, mapped rather than left alone, because a clipper
/// that passed one value of one polarity through would not be symmetric.
///
/// **What it does not do.** Clipping to a declared ceiling is **not** reported as
/// [`crate::dsp::DspObservation::Saturated`]. That observation names a value clamped to full scale
/// — a bound the arithmetic ran into — and a hard clipper's ceiling is the effect the caller asked
/// for. Nor does this processor apply drive or make-up gain, look ahead, or soften the corner: put
/// a [`super::level::Gain`] stage before it for drive and after it for level, and use
/// [`SoftClip`] when the corner is what matters.
#[derive(Debug, Clone)]
pub struct HardClip {
    body: Body,
    ceiling: i32,
}

impl Default for HardClip {
    fn default() -> Self {
        Self::new()
    }
}

impl HardClip {
    /// A clipper at full scale, which passes everything but `i16::MIN` through unchanged.
    #[must_use]
    pub fn new() -> Self {
        Self {
            body: Body::default(),
            ceiling: 32_767,
        }
    }

    fn declaration() -> DspCapability {
        DspCapability::new(super::HARD_CLIP)
            .with_channels(CHANNELS)
            .with_parameters(HARD_CLIP_PARAMETERS)
            .with_state_bytes(state_bytes::<Self>(0))
            .with_reset(ResetBehavior::Stateless)
            .with_execution(ExecutionPolicy::new(ExecutionProfile::ProvenInline))
    }
}

impl FrameProcessor for HardClip {
    fn capability(&self) -> DspCapability {
        Self::declaration()
    }

    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        let capability = self.capability();
        capability.validate_parameters(parameters)?;
        for parameter in parameters {
            if let ("ceiling", ParameterValue::Integer(value)) = (parameter.id(), parameter.value())
            {
                self.ceiling = i32::try_from(value).unwrap_or(i32::MAX);
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
            .prepare(&capability, direction, format)
    }

    fn process(
        &mut self,
        frame: &DspFrame<'_>,
        _scratch: &mut Scratch<'_>,
        sink: &mut FrameSink<'_>,
    ) -> Result<(), ProcessError> {
        let capability = self.capability();
        open(&mut self.body, &capability, frame, sink, &mut || {})?;
        let ceiling = self.ceiling;
        transform(frame, sink, |position| {
            for sample in position.iter_mut() {
                *sample = (*sample).clamp(-ceiling, ceiling);
            }
        })
    }

    fn flush(&mut self, _sink: &mut FrameSink<'_>) -> Result<(), ProcessError> {
        self.body.admission().admit_flush()
    }

    fn reset(&mut self, _cause: DspResetCause) {
        self.body.admission().reset();
    }

    fn cancel(&mut self) {
        self.body.admission().cancel();
    }

    fn retained(&self) -> u32 {
        0
    }
}

const SOFT_CLIP_PARAMETERS: &[ParameterSpec] = &[ParameterSpec::new(
    "threshold",
    ParameterDomain::Integer {
        min: 0,
        max: FULL_SCALE - 1,
    },
)];

/// A soft clipper with a quadratic knee.
///
/// **What it does.** Below `threshold` the transfer function is the identity. Above it, on the
/// magnitude `d = |x|` with `t = threshold` and `c = 32_767`:
///
/// ```text
/// y = d − (d − t)² / (4·(c − t))          for t < d
/// ```
///
/// truncating the division and re-applying the input's sign. The curve meets the identity at `t`
/// with a matching slope, so there is no corner where the knee begins, and it reaches its own
/// maximum of `c − (c − t)/4` at full-scale input. That is the whole point of a soft clipper and
/// it is stated here as arithmetic because "smooth" is not a testable word.
///
/// **What it does not do.** It never reaches full scale, so it never reports
/// [`crate::dsp::DspObservation::Saturated`] — and it therefore *reduces* peak level rather than
/// holding it, which is the price of the soft corner. It applies no drive and no make-up gain: put
/// a [`super::level::Gain`] stage on either side. It has no attack, release, envelope or
/// look-ahead — it is a memoryless transfer function and not a compressor — and its knee width is
/// fixed by `threshold` rather than being a second parameter.
#[derive(Debug, Clone)]
pub struct SoftClip {
    body: Body,
    threshold: i64,
}

impl Default for SoftClip {
    fn default() -> Self {
        Self::new()
    }
}

impl SoftClip {
    /// A clipper whose knee begins at half of full scale.
    #[must_use]
    pub fn new() -> Self {
        Self {
            body: Body::default(),
            threshold: 16_384,
        }
    }

    fn declaration() -> DspCapability {
        DspCapability::new(super::SOFT_CLIP)
            .with_channels(CHANNELS)
            .with_parameters(SOFT_CLIP_PARAMETERS)
            .with_state_bytes(state_bytes::<Self>(0))
            .with_reset(ResetBehavior::Stateless)
            .with_execution(ExecutionPolicy::new(ExecutionProfile::ProvenInline))
    }

    /// The knee, on a non-negative magnitude.
    fn knee(magnitude: i64, threshold: i64) -> i64 {
        if magnitude <= threshold {
            return magnitude;
        }
        let over = magnitude - threshold;
        let width = (FULL_SCALE - threshold).max(1);
        magnitude - over.saturating_mul(over) / width.saturating_mul(4)
    }
}

impl FrameProcessor for SoftClip {
    fn capability(&self) -> DspCapability {
        Self::declaration()
    }

    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        let capability = self.capability();
        capability.validate_parameters(parameters)?;
        for parameter in parameters {
            if let ("threshold", ParameterValue::Integer(value)) =
                (parameter.id(), parameter.value())
            {
                self.threshold = value;
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
            .prepare(&capability, direction, format)
    }

    fn process(
        &mut self,
        frame: &DspFrame<'_>,
        _scratch: &mut Scratch<'_>,
        sink: &mut FrameSink<'_>,
    ) -> Result<(), ProcessError> {
        let capability = self.capability();
        open(&mut self.body, &capability, frame, sink, &mut || {})?;
        let threshold = self.threshold;
        transform(frame, sink, |position| {
            for sample in position.iter_mut() {
                let value = i64::from(*sample);
                let shaped = Self::knee(value.abs(), threshold);
                *sample = narrow(if value < 0 { -shaped } else { shaped });
            }
        })
    }

    fn flush(&mut self, _sink: &mut FrameSink<'_>) -> Result<(), ProcessError> {
        self.body.admission().admit_flush()
    }

    fn reset(&mut self, _cause: DspResetCause) {
        self.body.admission().reset();
    }

    fn cancel(&mut self) {
        self.body.admission().cancel();
    }

    fn retained(&self) -> u32 {
        0
    }
}

const BIT_CRUSH_PARAMETERS: &[ParameterSpec] = &[
    ParameterSpec::new("bits", ParameterDomain::Integer { min: 1, max: 16 }),
    ParameterSpec::new(
        "hold_positions",
        ParameterDomain::Integer {
            min: 1,
            max: MAX_HOLD,
        },
    ),
];

/// A bit-depth and sample-rate reducer.
///
/// **What it does.** Two independent reductions, applied in that order:
///
/// - **Hold.** Each channel's sample is captured once every `hold_positions` positions and repeated
///   until the next capture. The counter runs in positions, not in frames, so re-cutting the same
///   stream produces the same samples.
/// - **Depth.** The held sample is floored onto a grid of `2^(16 − bits)` — an arithmetic shift
///   right and back — which is a mid-tread quantiser biased toward negative infinity. At the
///   default 16 bits the grid is the sample grid and the reduction is the identity.
///
/// **What it does not do.** It does not dither, noise-shape or filter. Quantisation noise is
/// correlated with the signal and aliases fold back unfiltered, which is what a bit crusher is for;
/// if that is not what is wanted, this is the wrong processor rather than the wrong settings. It
/// does not resample: `hold_positions` repeats samples at the stream's own rate and the frame's
/// position count is unchanged. And the floor is toward negative infinity rather than toward zero,
/// so a quiet negative signal quantises to `-2^(16 − bits)` rather than to silence.
#[derive(Debug, Clone)]
pub struct BitCrush {
    body: Body,
    bits: u32,
    hold: u32,
    remaining: u32,
    held: [i32; MAX_CHANNELS],
}

impl Default for BitCrush {
    fn default() -> Self {
        Self::new()
    }
}

impl BitCrush {
    /// A crusher at the full depth and no hold, which is the identity.
    #[must_use]
    pub fn new() -> Self {
        Self {
            body: Body::default(),
            bits: 16,
            hold: 1,
            remaining: 0,
            held: [0; MAX_CHANNELS],
        }
    }

    fn declaration() -> DspCapability {
        DspCapability::new(super::BIT_CRUSH)
            .with_channels(CHANNELS)
            .with_parameters(BIT_CRUSH_PARAMETERS)
            .with_state_bytes(state_bytes::<Self>(0))
            .with_reset(ResetBehavior::ClearsState)
            .with_execution(ExecutionPolicy::new(ExecutionProfile::ProvenInline))
    }

    fn clear(&mut self) {
        self.remaining = 0;
        self.held = [0; MAX_CHANNELS];
    }
}

impl FrameProcessor for BitCrush {
    fn capability(&self) -> DspCapability {
        Self::declaration()
    }

    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        let capability = self.capability();
        capability.validate_parameters(parameters)?;
        for parameter in parameters {
            match (parameter.id(), parameter.value()) {
                ("bits", ParameterValue::Integer(value)) => {
                    self.bits = u32::try_from(value).unwrap_or(16).clamp(1, 16);
                }
                ("hold_positions", ParameterValue::Integer(value)) => {
                    self.hold = u32::try_from(value).unwrap_or(1).max(1);
                }
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
        self.clear();
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
            let remaining = &mut self.remaining;
            let held = &mut self.held;
            open(&mut self.body, &capability, frame, sink, &mut || {
                *remaining = 0;
                *held = [0; MAX_CHANNELS];
            })?;
        }
        let shift = 16u32.saturating_sub(self.bits);
        let period = self.hold;
        let remaining = &mut self.remaining;
        let captured = &mut self.held;
        transform(frame, sink, |position| {
            if *remaining == 0 {
                for (slot, sample) in captured.iter_mut().zip(position.iter()) {
                    *slot = *sample;
                }
                *remaining = period;
            }
            *remaining = remaining.saturating_sub(1);
            for (sample, slot) in position.iter_mut().zip(captured.iter()) {
                *sample = narrow((i64::from(*slot) >> shift) << shift);
            }
        })
    }

    fn flush(&mut self, _sink: &mut FrameSink<'_>) -> Result<(), ProcessError> {
        self.body.admission().admit_flush()
    }

    fn reset(&mut self, _cause: DspResetCause) {
        self.body.admission().reset();
        self.clear();
    }

    fn cancel(&mut self) {
        self.body.admission().cancel();
        self.clear();
    }

    fn retained(&self) -> u32 {
        0
    }
}
