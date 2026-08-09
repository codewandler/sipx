//! Level: gain and polarity (`docs/specs/call-dsp-effects.md` §5).

use crate::analysis::AudioDirection;
use crate::dsp::contract::{
    DspCapability, DspFrame, DspResetCause, ExecutionPolicy, ExecutionProfile, FormatError,
    FrameProcessor, FrameSink, Parameter, ParameterDomain, ParameterError, ParameterSpec,
    ParameterValue, ProcessError, ResetBehavior, Scratch, StreamFormat,
};

use super::arithmetic::{Ramp, narrow, scaled};
use super::frame::{Body, CHANNELS, open, state_bytes, transform};

/// The largest gain this processor admits, in thousandths: 8.0, or +18.06 dB.
const MAX_GAIN: i32 = 8_000;

/// The longest gain transition this processor admits, in positions.
const MAX_SMOOTHING: i64 = 4_096;

const GAIN_PARAMETERS: &[ParameterSpec] = &[
    ParameterSpec::new(
        "gain",
        ParameterDomain::Ratio {
            min: 0,
            max: MAX_GAIN,
        },
    ),
    ParameterSpec::new(
        "smoothing_positions",
        ParameterDomain::Integer {
            min: 0,
            max: MAX_SMOOTHING,
        },
    ),
];

/// A saturating amplitude gain, optionally ramped over a declared number of positions.
///
/// **What it does.** Multiplies every sample by `gain`, a ratio in thousandths from 0.0 through
/// 8.0, rounding half away from zero so that a signal and its negation stay exact negations of one
/// another. A change to `gain` is applied over `smoothing_positions` positions as a linear ramp
/// that lands exactly on the target; at the default of 0 positions it is applied whole at the first
/// position of the next frame. The ramp is a function of position and not of framing, so cutting
/// the same stream differently produces the same samples.
///
/// A transition survives a reset. A ramp is parameter state, and §8.1 keeps declared parameters
/// across a reset while discarding sample memory — of which this processor holds none, which is why
/// it declares [`ResetBehavior::Stateless`] and a reset of it is unobservable.
///
/// **What it does not do.** It does not invert (that is [`Polarity`]), does not limit or compress
/// (that is [`super::shape::HardClip`] and [`super::shape::SoftClip`]), does not follow the signal
/// — there is no envelope, no detector and no automatic level — and it does not dither: a gain
/// below unity truncates through the same rounding as one above it. Values past full scale are
/// clamped and counted as [`crate::dsp::DspObservation::Saturated`]; a gain of 8.0 on ordinary
/// speech will clamp, and that is the declared behaviour rather than a defect.
#[derive(Debug, Clone)]
pub struct Gain {
    body: Body,
    ramp: Ramp,
    smoothing: u32,
}

impl Default for Gain {
    fn default() -> Self {
        Self::new()
    }
}

impl Gain {
    /// A gain at unity, applied whole.
    #[must_use]
    pub fn new() -> Self {
        Self {
            body: Body::default(),
            ramp: Ramp::new(1_000),
            smoothing: 0,
        }
    }

    fn declaration() -> DspCapability {
        DspCapability::new(super::GAIN)
            .with_channels(CHANNELS)
            .with_parameters(GAIN_PARAMETERS)
            .with_state_bytes(state_bytes::<Self>(0))
            .with_reset(ResetBehavior::Stateless)
            .with_execution(ExecutionPolicy::new(ExecutionProfile::ProvenInline))
    }
}

impl FrameProcessor for Gain {
    fn capability(&self) -> DspCapability {
        Self::declaration()
    }

    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        let capability = self.capability();
        capability.validate_parameters(parameters)?;

        // The whole set is read before any of it is applied, so a `gain` written before a
        // `smoothing_positions` in the same set still ramps over the new length.
        let mut gain = None;
        let mut smoothing = self.smoothing;
        for parameter in parameters {
            match (parameter.id(), parameter.value()) {
                ("gain", ParameterValue::Ratio(value)) => gain = Some(value),
                ("smoothing_positions", ParameterValue::Integer(value)) => {
                    smoothing = u32::try_from(value).unwrap_or(0);
                }
                _ => {}
            }
        }
        self.smoothing = smoothing;
        if let Some(target) = gain {
            self.ramp.to(target, smoothing);
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
        let ramp = &mut self.ramp;
        transform(frame, sink, |position| {
            let gain = i64::from(ramp.advance());
            for sample in position.iter_mut() {
                *sample = narrow(scaled(i64::from(*sample), gain, 1_000));
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

const POLARITY_PARAMETERS: &[ParameterSpec] =
    &[ParameterSpec::new("inverted", ParameterDomain::Flag)];

/// A polarity inversion.
///
/// **What it does.** Negates every sample when `inverted` is set, and is the exact identity when it
/// is not. `i16::MIN` has no positive counterpart in the representable range, so it clamps to
/// `i16::MAX` and the frame reports [`crate::dsp::DspObservation::Saturated`] — the one place the
/// asymmetry of two's complement is audible, made visible rather than wrapped around.
///
/// **What it does not do.** It does not fade, cross-fade or ramp: a polarity change is applied
/// whole at the first position of the next frame, and it declares no `smoothing_positions` because
/// interpolating between a signal and its negation is a cross-fade through silence — a different
/// effect, with a different name, which this story does not ship. It also does not align channels:
/// on a stereo stream both channels invert together, and per-channel polarity is not a parameter
/// here.
#[derive(Debug, Clone, Default)]
pub struct Polarity {
    body: Body,
    inverted: bool,
}

impl Polarity {
    /// A polarity that passes its input through.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn declaration() -> DspCapability {
        DspCapability::new(super::POLARITY)
            .with_channels(CHANNELS)
            .with_parameters(POLARITY_PARAMETERS)
            .with_state_bytes(state_bytes::<Self>(0))
            .with_reset(ResetBehavior::Stateless)
            .with_execution(ExecutionPolicy::new(ExecutionProfile::ProvenInline))
    }
}

impl FrameProcessor for Polarity {
    fn capability(&self) -> DspCapability {
        Self::declaration()
    }

    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        let capability = self.capability();
        capability.validate_parameters(parameters)?;
        for parameter in parameters {
            if let ("inverted", ParameterValue::Flag(value)) = (parameter.id(), parameter.value()) {
                self.inverted = value;
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
        let inverted = self.inverted;
        transform(frame, sink, |position| {
            if inverted {
                for sample in position.iter_mut() {
                    *sample = narrow(-i64::from(*sample));
                }
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
