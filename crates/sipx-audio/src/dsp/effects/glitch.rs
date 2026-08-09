//! The bounded delay and stutter line (`docs/specs/call-dsp-effects.md` §7).

use crate::analysis::AudioDirection;
use crate::dsp::contract::{
    CapabilityError, DspCapability, DspFrame, DspResetCause, ExecutionPolicy, ExecutionProfile,
    FormatError, FrameProcessor, FrameSink, Parameter, ParameterDomain, ParameterError,
    ParameterSpec, ParameterValue, ProcessError, ResetBehavior, Scratch, StreamFormat,
};

use super::arithmetic::clamp_sample;
use super::frame::{Body, CHANNELS, MAX_CHANNELS, open, state_bytes, transform};

/// The longest line [`Stutter`] admits, in positions.
///
/// It is the graph contract's own default `retained_tail_positions`, reused rather than re-minted,
/// so a line at its maximum is exactly what a default-bounded graph will still attach. At 8,000 Hz
/// that is 512 ms and at 48,000 Hz it is 85 ms.
pub const MAX_STUTTER_POSITIONS: u32 = 4_096;

const STUTTER_PARAMETERS: &[ParameterSpec] = &[ParameterSpec::new("repeat", ParameterDomain::Flag)];

/// A bounded delay line that can be told to loop what it holds.
///
/// **What it does.** Holds exactly the number of positions its constructor was given, fixed for the
/// life of the instance and declared as both `latency_positions` and `tail_positions` so that a
/// graph can sum it without simulating it. With `repeat` clear it is a pure delay: output position
/// *p* carries input position *p − d*, one position out for one in. With `repeat` set it stops
/// taking new input and reads the line cyclically, so the last *d* positions loop for as long as
/// the flag is set — which is the effect. Clearing the flag resumes the delay from whatever the
/// line still holds. `flush` writes the line out rather than dropping it; a reset discards it,
/// because retained audio belongs to an epoch that no longer exists.
///
/// **What it does not do — and this is the sentence the whole effect exists to keep true.** A
/// repeat is something the application asked for, and it says so as a
/// [`ParameterApplied`](crate::dsp::DspObservation::ParameterApplied) observation naming `repeat`
/// and the position it took effect at. It is **not** a discontinuity: a break the seam declared
/// arrives as a [`DiscontinuityKind`](crate::analysis::DiscontinuityKind) on the frame and leaves a
/// [`Restarted`](crate::dsp::DspObservation::Restarted) observation behind, and this processor
/// never emits one of those for a repeat. A deadline miss is neither — it is a fact the runtime
/// observed about a processor rather than one a processor observed about a frame, and
/// `docs/specs/call-dsp-graph.md` §5.3 reports it as a `Bypassed` transition that no processor can
/// produce. Three vocabularies, no overlap; an overload defect is never presented as this effect
/// and this effect is never counted as an overload defect.
///
/// It also does not fade, cross-fade or window the loop point, so a repeat whose line does not
/// contain a whole number of periods clicks at the seam; does not vary the line length while
/// running, because latency and tail are declarations a graph has already sized buffers from; and
/// does not feed back — the line is written once from the input and never from its own output, so
/// there is no repeat count, no decay and no unbounded growth.
#[derive(Debug, Clone)]
pub struct Stutter {
    body: Body,
    line: Vec<i16>,
    positions: u32,
    channels: usize,
    cursor: usize,
    filled: u32,
    repeat: bool,
}

impl Stutter {
    /// A line of exactly `delay_positions`.
    ///
    /// # Errors
    ///
    /// [`CapabilityError::Field`] past [`MAX_STUTTER_POSITIONS`]. A line of 0 positions is
    /// admissible and is a pass-through: it is the boundary case an application reaches by
    /// configuring the effect away, and refusing it would make "zero delay" a special case callers
    /// have to know about.
    pub fn new(delay_positions: u32) -> Result<Self, CapabilityError> {
        if delay_positions > MAX_STUTTER_POSITIONS {
            return Err(CapabilityError::Field {
                field: "delay_positions",
                value: i64::from(delay_positions),
            });
        }
        let samples = usize::try_from(delay_positions)
            .unwrap_or(0)
            .saturating_mul(MAX_CHANNELS);
        Ok(Self {
            body: Body::default(),
            line: vec![0; samples],
            positions: delay_positions,
            channels: 1,
            cursor: 0,
            filled: 0,
            repeat: false,
        })
    }

    /// The declaration, which is a function of the line this instance was built with.
    fn declaration(&self) -> DspCapability {
        let heap = u64::from(self.positions)
            .saturating_mul(u64::try_from(MAX_CHANNELS).unwrap_or(0))
            .saturating_mul(2);
        DspCapability::new(super::STUTTER)
            .with_channels(CHANNELS)
            .with_parameters(STUTTER_PARAMETERS)
            .with_state_bytes(state_bytes::<Self>(heap))
            .with_latency_positions(self.positions)
            .with_tail_positions(self.positions)
            .with_reset(ResetBehavior::ClearsState)
            .with_execution(ExecutionPolicy::new(ExecutionProfile::ProvenInline))
    }

    /// Discard everything the line holds, without emitting it.
    fn clear(line: &mut [i16], cursor: &mut usize, filled: &mut u32) {
        line.fill(0);
        *cursor = 0;
        *filled = 0;
    }
}

impl FrameProcessor for Stutter {
    fn capability(&self) -> DspCapability {
        self.declaration()
    }

    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        let capability = self.capability();
        capability.validate_parameters(parameters)?;
        for parameter in parameters {
            if let ("repeat", ParameterValue::Flag(value)) = (parameter.id(), parameter.value()) {
                self.repeat = value;
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
        self.channels = usize::from(format.channels()).clamp(1, MAX_CHANNELS);
        Self::clear(&mut self.line, &mut self.cursor, &mut self.filled);
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
            let line = &mut self.line;
            let cursor = &mut self.cursor;
            let filled = &mut self.filled;
            open(&mut self.body, &capability, frame, sink, &mut || {
                Self::clear(line, cursor, filled);
            })?;
        }
        if self.positions == 0 {
            return transform(frame, sink, |_| {});
        }

        let span = usize::try_from(self.positions).unwrap_or(0).max(1);
        let repeat = self.repeat;
        let limit = self.positions;
        let line = &mut self.line;
        let cursor = &mut self.cursor;
        let filled = &mut self.filled;
        transform(frame, sink, |position| {
            let offset = cursor.saturating_mul(MAX_CHANNELS);
            for (channel, sample) in position.iter_mut().enumerate() {
                let Some(slot) = line.get_mut(offset.saturating_add(channel)) else {
                    continue;
                };
                let held = i32::from(*slot);
                if !repeat {
                    let (limited, _) = clamp_sample(i64::from(*sample));
                    *slot = limited;
                }
                *sample = held;
            }
            *cursor = cursor.saturating_add(1) % span;
            if !repeat {
                *filled = filled.saturating_add(1).min(limit);
            }
        })
    }

    fn flush(&mut self, sink: &mut FrameSink<'_>) -> Result<(), ProcessError> {
        self.body.admission().admit_flush()?;
        let span = usize::try_from(self.positions).unwrap_or(0).max(1);
        let held = usize::try_from(self.filled).unwrap_or(0).min(span);
        // The oldest position the line still owes is `filled` steps behind the write cursor.
        let start = self.cursor.saturating_add(span).saturating_sub(held) % span;
        for step in 0..held {
            let slot = start.saturating_add(step) % span;
            let offset = slot.saturating_mul(MAX_CHANNELS);
            for channel in 0..self.channels {
                let sample = self
                    .line
                    .get(offset.saturating_add(channel))
                    .copied()
                    .unwrap_or(0);
                sink.push(sample)?;
            }
        }
        Self::clear(&mut self.line, &mut self.cursor, &mut self.filled);
        Ok(())
    }

    fn reset(&mut self, _cause: DspResetCause) {
        self.body.admission().reset();
        Self::clear(&mut self.line, &mut self.cursor, &mut self.filled);
    }

    fn cancel(&mut self) {
        self.body.admission().cancel();
        // §8.4: all sample memory is released. A line this instance owns can actually be given
        // back rather than merely zeroed, and cancellation is terminal, so it is.
        self.line = Vec::new();
        self.cursor = 0;
        self.filled = 0;
    }

    fn retained(&self) -> u32 {
        self.filled
    }
}
