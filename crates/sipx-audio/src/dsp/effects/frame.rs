//! The frame handling every built-in shares: `docs/specs/call-dsp-effects.md` §2.
//!
//! Admission, the discontinuity reset, parameter application and the per-position write are the
//! same obligations for every processor in this module, and the processor contract's §8.3 wants
//! each of them implemented identically — "typed refusal with no partial state mutation" is exactly
//! the sentence that is easy to get subtly wrong once per implementation. So each built-in supplies
//! its own transfer function and nothing else: [`open`] runs the prologue and [`transform`] runs the
//! loop, and neither is reachable from outside the crate.

use crate::dsp::contract::{
    Admitted, DspCapability, DspFrame, DspObservation, DspResetCause, FrameAdmission, FrameSink,
    MAX_PARAMETERS, Parameter, ProcessError,
};

use super::arithmetic::clamp_sample;

/// The channel counts every built-in accepts: mono and stereo.
///
/// The contract admits eight, and this module claims two. Those are the call paths this epic
/// serves; eight is the contract's headroom, and a declaration is a promise a caller sizes buffers
/// from rather than a place to be generous. Nothing here measures a third channel, so nothing here
/// declares one.
pub(super) const CHANNELS: &[u8] = &[1, 2];

/// The widest interleaved position [`CHANNELS`] admits.
pub(super) const MAX_CHANNELS: usize = 2;

/// Which declared parameters an accepted set assigned, waiting for the position they take effect at.
///
/// `configure` has no sink, so the [`DspObservation::ParameterApplied`] a set earns is emitted at
/// the first position of the first frame that follows it — which is §3.6's "a set is applied at one
/// position boundary or not at all", read as the boundary the caller can actually observe. They are
/// emitted in **declaration order** rather than in the order the caller wrote them, because §3.6
/// leaves ordering inside one set to the caller and an observation sequence that inherited it would
/// be one more thing two callers could disagree about.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct Pending {
    marked: u32,
}

impl Pending {
    /// Record an accepted set. Never called for a refused one: a refusal changes nothing.
    pub(super) fn mark(&mut self, capability: &DspCapability, parameters: &[Parameter]) {
        for parameter in parameters {
            let found = capability
                .parameters()
                .iter()
                .position(|spec| spec.id() == parameter.id());
            if let Some(Ok(bit)) = found.map(u32::try_from) {
                self.marked |= 1u32.checked_shl(bit).unwrap_or(0);
            }
        }
    }

    /// Emit one observation per assigned parameter at the position the set took effect.
    pub(super) fn drain(
        &mut self,
        capability: &DspCapability,
        at_position: u64,
        sink: &mut FrameSink<'_>,
    ) {
        if self.marked == 0 {
            return;
        }
        for (index, spec) in capability
            .parameters()
            .iter()
            .take(MAX_PARAMETERS)
            .enumerate()
        {
            let bit = u32::try_from(index).map(|bit| 1u32.checked_shl(bit).unwrap_or(0));
            if bit.is_ok_and(|bit| self.marked & bit != 0) {
                sink.emit(DspObservation::ParameterApplied {
                    parameter: spec.id(),
                    at_position,
                });
            }
        }
        self.marked = 0;
    }
}

/// The admission state and the parameter accounting every built-in owns.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct Body {
    admission: FrameAdmission,
    pending: Pending,
}

impl Body {
    /// The shared admission state machine, for a processor's own `prepare` and `flush`.
    pub(super) fn admission(&mut self) -> &mut FrameAdmission {
        &mut self.admission
    }

    /// Record an accepted parameter set (see [`Pending`]).
    pub(super) fn mark(&mut self, capability: &DspCapability, parameters: &[Parameter]) {
        self.pending.mark(capability, parameters);
    }
}

/// Admit a frame, run the reset its declared break demands, and apply any parameter set waiting.
///
/// The order is the order things happened: a break precedes the frame, so its
/// [`DspObservation::Restarted`] precedes the frame's own observations; a parameter set applies at
/// this frame's first position, so it follows the restart and precedes anything the samples earn.
/// `clear` is the processor's own sample memory, discarded **before** the flagged frame's samples
/// are consumed — those samples open the new epoch (§8.1).
///
/// # Errors
///
/// Every refusal in §8.3 that admission owns. A refusal writes nothing and mutates nothing, so a
/// caller that fixes its input and retries continues exactly where the stream stood.
pub(super) fn open(
    body: &mut Body,
    capability: &DspCapability,
    frame: &DspFrame<'_>,
    sink: &mut FrameSink<'_>,
    clear: &mut dyn FnMut(),
) -> Result<Admitted, ProcessError> {
    let admitted = body.admission.admit(capability, frame)?;
    if let Some(kind) = admitted.discontinuity() {
        clear();
        sink.emit(DspObservation::Restarted {
            cause: DspResetCause::Discontinuity { kind },
        });
    }
    body.pending.drain(capability, frame.position(), sink);
    Ok(admitted)
}

/// Write one frame, position by position, and report what the arithmetic did.
///
/// `step` is handed one position's channels as `i32`, widened from the frame and narrowed back
/// here. Everything that is the same for every effect lives on this side of it: the interleaving,
/// the saturation accounting and the two observations §6 admits about a frame's samples.
///
/// [`DspObservation::Saturated`] and [`DspObservation::PassedThrough`] are exclusive, and
/// deliberately: a frame that had to be clamped was not passed through, even where clamping
/// happened to return the value it started at.
///
/// # Errors
///
/// [`ProcessError::OutputOverflow`] if the sink is smaller than the frame's own position count,
/// which is a caller that did not size it with `max_output_positions`.
pub(super) fn transform<F>(
    frame: &DspFrame<'_>,
    sink: &mut FrameSink<'_>,
    mut step: F,
) -> Result<(), ProcessError>
where
    F: FnMut(&mut [i32]),
{
    let channels = usize::from(frame.format().channels());
    if channels == 0 || channels > MAX_CHANNELS {
        return Err(ProcessError::MalformedFrame);
    }
    let mut work = [0i32; MAX_CHANNELS];
    let mut out = [0i16; MAX_CHANNELS];
    let mut saturated: u32 = 0;
    let mut unchanged = true;

    for input in frame.samples().chunks_exact(channels) {
        let (Some(values), Some(written)) = (work.get_mut(..channels), out.get_mut(..channels))
        else {
            return Err(ProcessError::MalformedFrame);
        };
        for (value, sample) in values.iter_mut().zip(input) {
            *value = i32::from(*sample);
        }
        step(values);

        let mut clamped = false;
        for ((slot, value), sample) in written.iter_mut().zip(values.iter()).zip(input) {
            let (limited, hit) = clamp_sample(i64::from(*value));
            clamped |= hit;
            unchanged &= limited == *sample;
            *slot = limited;
        }
        if clamped {
            saturated = saturated.saturating_add(1);
        }
        sink.write(written)?;
    }

    if saturated > 0 {
        sink.emit(DspObservation::Saturated {
            positions: saturated,
        });
    } else if unchanged {
        sink.emit(DspObservation::PassedThrough);
    }
    Ok(())
}

/// A truthful `state_bytes` for a processor: what it holds inline, plus what it owns on the heap.
///
/// §9.1 is exact about what this figure is and is not. The conformance harness measures the inline
/// half against this declaration and reports the heap half `Unproven` by name, because
/// `unsafe_code` is forbidden workspace-wide and a counting allocator cannot be installed — so a
/// processor that owns a delay line states its size here rather than leaving it uncounted.
pub(super) fn state_bytes<T>(heap: u64) -> u64 {
    u64::try_from(size_of::<T>())
        .unwrap_or(u64::MAX)
        .saturating_add(heap)
}
