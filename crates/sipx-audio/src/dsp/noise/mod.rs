//! Interchangeable noise reduction: `docs/specs/call-dsp-noise-reduction.md` (`M-66`).
//!
//! A **noise reducer** is a [`FrameProcessor`] that attenuates part of
//! its input on an estimate it derives from that same input, and declares [`NoiseReduction`]'s
//! three extra facts about how it does so. It is the same trait, the same frames, the same
//! refusals, the same observations and the same conformance harness as every other processor in
//! [`crate::dsp`]; what it adds is a declaration, not a door.
//!
//! # The interface is the deliverable
//!
//! [`NoiseReducer`] adds exactly one method to the processor contract, and everything a reducer
//! does to a frame stays [`crate::dsp`]'s. That is what lets a call layer name the trait and never
//! the implementation: `sipx-media`'s graph takes a `FrameProcessor`, a reducer reaches it as one,
//! and neither the graph nor the seam nor the session can tell which reducer is installed.
//!
//! One implementation ships — [`SubbandSuppressor`] — and a second, deliberately different one is
//! written from outside this crate in `tests/dsp_noise_reduction.rs`, because a single
//! implementation behind an interface proves only that the interface compiles. Both are driven
//! through one generic harness and both are held to `DSP-K1`..`DSP-K12` with the same unproven set.
//!
//! # What this is not
//!
//! Four neighbours are named apart, because conflating any of them with noise reduction would be a
//! claim about a call nobody could check:
//!
//! - **Voice activity detection** is [`crate::analysis`]'s and stays there. A reducer never emits,
//!   redefines, delays or suppresses a VAD observation. It may *consume* activity as a declared
//!   parameter the caller sets ([`ActivityInput::Optional`]) and never as a call into an analyser.
//!   [`ActivityHint`] is that caller's side of the arrangement (`M-114`, §10): it turns drained
//!   observations into the declared flag, holds no reducer and no analyser, and is a *policy* —
//!   §5.6's fifth row makes a wrong hint worse than no hint in both directions, so what decides
//!   when the flag is set is measured before it is recommended.
//! - **Echo cancellation** is not here and is not implied: an echo canceller needs the far-end
//!   reference signal, which is the other direction of the call, and a processor is bound to one
//!   direction at `prepare`.
//! - **Speech recognition and synthesis** are the provider contracts in
//!   `docs/specs/speech-providers.md`. A reducer produces samples, never text.
//! - **Automatic gain control, dereverberation and source separation** are not here either. Every
//!   gain a reducer applies is at most unity, so nothing it does can make a quiet talker louder.
//!
//! ```
//! use sipx_audio::analysis::AudioDirection;
//! use sipx_audio::dsp::noise::{ActivityInput, HostRequirement, NoiseReducer, SubbandSuppressor};
//! use sipx_audio::dsp::{FrameProcessor, StreamFormat};
//!
//! let reducer = SubbandSuppressor::new();
//! let declaration = reducer.noise_reduction();
//! declaration.validate()?;
//!
//! // Everything a caller needs to size a call is read, never guessed.
//! assert_eq!(declaration.warm_up_positions(), 1_024);
//! assert_eq!(declaration.processor().latency_positions(), 0);
//! assert_eq!(declaration.host_requirement(), HostRequirement::PortableInteger);
//! assert_eq!(
//!     declaration.activity_input(),
//!     ActivityInput::Optional { parameter: "voice_active" },
//! );
//! assert_eq!(declaration.admits_host(&[]), Ok(()));
//!
//! // A rate it was never shaped for is refused rather than resampled.
//! let mut reducer = reducer;
//! assert!(reducer.prepare(AudioDirection::Inbound, StreamFormat::new(44_100, 1)?).is_err());
//! assert!(reducer.prepare(AudioDirection::Inbound, StreamFormat::new(8_000, 1)?).is_ok());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod hint;
mod subband;

pub use hint::{ActivityHint, DEFAULT_HOLD_POSITIONS, HintCause, HintChange, HintPolicy};
pub use subband::SubbandSuppressor;

use super::contract::{CapabilityError, DspCapability, FrameProcessor, ParameterDomain};

/// The longest warm-up §3 admits, in positions.
///
/// This is [`crate::dsp::MAX_LATENCY_POSITIONS`]'s value, reused. A warm-up is not a buffer, but it
/// is a position count a caller reasons about beside latency and tail, and a second ceiling would
/// be a second number to keep consistent for no gain.
pub const MAX_WARM_UP_POSITIONS: u32 = super::contract::MAX_LATENCY_POSITIONS;

/// [`SubbandSuppressor`]'s declared identifier.
pub const SUBBAND_SUPPRESSOR: &str = "sipx.subband_suppressor";

/// Every noise reducer this workspace ships, by declared identifier, in a stable order.
///
/// It is part of the contract in the way [`crate::dsp::CHECKS`] is: a caller asserting on one of
/// these strings is asserting on a name, so the names do not move. It is deliberately separate from
/// [`crate::dsp::effects::BUILT_IN_IDS`] — that list is `M-65`'s nine effects and filters, declared
/// normatively in its own spec, and a reducer is not one of them.
pub const NOISE_REDUCTION_IDS: &[&str] = &[SUBBAND_SUPPRESSOR];

/// Whether a reducer consumes a voice-activity hint, and under which parameter (§3.3).
///
/// The claim is checkable rather than prose: [`NoiseReduction::validate`] requires an
/// [`Self::Optional`] parameter to appear in the processor's own declared schema with domain
/// [`ParameterDomain::Flag`], so a reducer that says it consumes activity and declares no such flag
/// is refused at the declaration, before a caller wires a detector to a parameter that is not there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ActivityInput {
    /// The reducer never consumes voice activity, and a caller wiring a detector to it is wiring it
    /// to nothing.
    Ignored,
    /// The reducer consumes an activity hint through this declared flag.
    ///
    /// **Optional means optional**: a reducer under this variant is deterministic and useful with
    /// the flag never set, and never refuses a frame for want of one. What the flag buys is the one
    /// thing an estimator cannot do for itself — refusing to adapt into speech it cannot recognise
    /// — and a hint from a detector that is wrong half the time is worse than no hint at all.
    Optional {
        /// The declared `Flag` parameter the hint arrives through.
        parameter: &'static str,
    },
}

/// What a reducer needs from the machine it runs on (§3.2).
///
/// This is a **declaration and never a probe**. `docs/specs/custom-call-dsp.md` §4.1 forbids device
/// enumeration outright — "a processor that wants to know whether a machine has a particular
/// instruction set asks its declared parameters, not the machine" — so a reducer states what it
/// needs, the caller states what it has, and [`NoiseReduction::admits_host`] compares the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HostRequirement {
    /// Integer arithmetic and nothing else: no instruction-set extension, no accelerator, no
    /// coprocessor, no device and no model of any kind. Every machine this crate compiles for runs
    /// it identically, which is the same sentence as "its vectors are portable".
    PortableInteger,
    /// The reducer needs a named host facility.
    ///
    /// Nothing in this workspace declares one. The variant exists so that an implementation which
    /// does can say so and be refused cleanly at [`NoiseReduction::admits_host`], rather than
    /// discovering it at the first frame on the media worker.
    Device {
        /// The facility's name, as the caller's own inventory spells it.
        id: &'static str,
    },
}

/// Why a noise-reduction declaration was refused (§3.4), or why a host may not run it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum NoiseReductionError {
    /// The processor half of the declaration is not internally admissible.
    ///
    /// The contract's own error, carried rather than re-minted: a reducer's capability is refused
    /// for exactly the reasons every other processor's is.
    #[error(transparent)]
    Capability(#[from] CapabilityError),
    /// The warm-up is longer than [`MAX_WARM_UP_POSITIONS`].
    #[error("a warm-up of {positions} positions is past the {MAX_WARM_UP_POSITIONS} ceiling")]
    WarmUp {
        /// What was declared.
        positions: u32,
    },
    /// The declaration claims an activity input the processor's schema does not declare.
    #[error("this reducer claims the activity parameter `{parameter}`, which it does not declare")]
    ActivityParameterUndeclared {
        /// The parameter claimed.
        parameter: &'static str,
    },
    /// The declared activity parameter exists but is not a flag, so a hint could not be given.
    #[error("the activity parameter `{parameter}` is declared, but its domain is not a flag")]
    ActivityParameterNotAFlag {
        /// The parameter claimed.
        parameter: &'static str,
    },
    /// A [`HostRequirement::Device`] carries an empty name, which no caller could satisfy.
    #[error("this reducer requires a device with an empty name")]
    EmptyDeviceIdentifier,
    /// The caller's host does not offer the device this reducer requires.
    #[error("this reducer requires the device `{required}`, which this host does not offer")]
    DeviceUnavailable {
        /// The device the declaration names.
        required: &'static str,
    },
}

/// Everything a noise reducer declares (§3).
///
/// A processor already declares its rates, channels, frame ceiling, scratch, latency, tail, length
/// policy, reset behaviour, execution policy and parameter schema — all of it
/// [`DspCapability`], carried here rather than restated. This adds the three facts a caller
/// choosing between two reducers cannot answer from that alone: how long the estimate takes to
/// become usable, whether the reducer consumes a voice-activity hint, and what it needs from the
/// machine.
///
/// **The warm-up is a position count and never a duration.** It opens at `prepare` and re-opens at
/// every reset and every discontinuity — `docs/specs/custom-call-dsp.md` §8.1's epoch, and not a
/// second notion of one. That a reducer therefore warms up in half the wall-clock time on a
/// 16,000 Hz call as on an 8,000 Hz one is a consequence of sample position being the only clock a
/// processor has, and it is stated rather than hidden: a caller wanting equal wall-clock behaviour
/// across rates configures the rate-dependent parameters per rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoiseReduction {
    processor: DspCapability,
    warm_up_positions: u32,
    activity: ActivityInput,
    host: HostRequirement,
}

impl NoiseReduction {
    /// The declaration for a processor that warms up instantly, consumes no activity and needs
    /// nothing but integer arithmetic.
    ///
    /// Every `with_*` method changes one field. Nothing is validated here: a declaration is data,
    /// and [`Self::validate`] checks it once, before anything is sized by it.
    #[must_use]
    pub const fn new(processor: DspCapability) -> Self {
        Self {
            processor,
            warm_up_positions: 0,
            activity: ActivityInput::Ignored,
            host: HostRequirement::PortableInteger,
        }
    }

    /// How many positions of an epoch the estimate takes to become usable.
    #[must_use]
    pub const fn with_warm_up_positions(mut self, positions: u32) -> Self {
        self.warm_up_positions = positions;
        self
    }

    /// Whether a voice-activity hint is consumed, and through which declared flag.
    #[must_use]
    pub const fn with_activity_input(mut self, activity: ActivityInput) -> Self {
        self.activity = activity;
        self
    }

    /// What this reducer needs from the machine it runs on.
    #[must_use]
    pub const fn with_host_requirement(mut self, host: HostRequirement) -> Self {
        self.host = host;
        self
    }

    /// The processor half of the declaration: rates, channels, frames, latency, tail, parameters.
    #[must_use]
    pub const fn processor(&self) -> DspCapability {
        self.processor
    }

    /// How many positions of an epoch the estimate takes to become usable.
    #[must_use]
    pub const fn warm_up_positions(&self) -> u32 {
        self.warm_up_positions
    }

    /// Whether a voice-activity hint is consumed, and through which declared flag.
    #[must_use]
    pub const fn activity_input(&self) -> ActivityInput {
        self.activity
    }

    /// What this reducer needs from the machine it runs on.
    #[must_use]
    pub const fn host_requirement(&self) -> HostRequirement {
        self.host
    }

    /// Whether this declaration is internally admissible (§3.4).
    ///
    /// # Errors
    ///
    /// [`NoiseReductionError::Capability`] for anything `docs/specs/custom-call-dsp.md` §5 refuses,
    /// [`NoiseReductionError::WarmUp`] past [`MAX_WARM_UP_POSITIONS`],
    /// [`NoiseReductionError::ActivityParameterUndeclared`] and
    /// [`NoiseReductionError::ActivityParameterNotAFlag`] for an activity claim the schema does not
    /// back, and [`NoiseReductionError::EmptyDeviceIdentifier`] for a device nobody could name.
    pub fn validate(&self) -> Result<(), NoiseReductionError> {
        self.processor.validate()?;
        if self.warm_up_positions > MAX_WARM_UP_POSITIONS {
            return Err(NoiseReductionError::WarmUp {
                positions: self.warm_up_positions,
            });
        }
        if let ActivityInput::Optional { parameter } = self.activity {
            let declared = self
                .processor
                .parameters()
                .iter()
                .find(|spec| spec.id() == parameter);
            let Some(spec) = declared else {
                return Err(NoiseReductionError::ActivityParameterUndeclared { parameter });
            };
            if !matches!(spec.domain(), ParameterDomain::Flag) {
                return Err(NoiseReductionError::ActivityParameterNotAFlag { parameter });
            }
        }
        if matches!(self.host, HostRequirement::Device { id } if id.is_empty()) {
            return Err(NoiseReductionError::EmptyDeviceIdentifier);
        }
        Ok(())
    }

    /// Whether a host offering `devices` may run this reducer (§3.2).
    ///
    /// Nothing is probed: `devices` is the caller's own inventory, and a reducer that needs
    /// something absent from it is refused here rather than at the first frame.
    ///
    /// # Errors
    ///
    /// [`NoiseReductionError::DeviceUnavailable`] when the declared device is not offered.
    pub fn admits_host(&self, devices: &[&str]) -> Result<(), NoiseReductionError> {
        match self.host {
            HostRequirement::PortableInteger => Ok(()),
            HostRequirement::Device { id } => {
                if devices.contains(&id) {
                    Ok(())
                } else {
                    Err(NoiseReductionError::DeviceUnavailable { required: id })
                }
            }
        }
    }
}

/// A [`FrameProcessor`] that attenuates part of its input on an estimate it derives from it (§2).
///
/// The trait adds exactly one method, and that is the whole of the interface: everything a reducer
/// does to a frame is the processor contract's, unchanged. A call layer therefore names this trait
/// or its supertrait and never an implementation, which is what makes substituting one reducer for
/// another invisible to a call.
///
/// **What an implementation promises**, beyond [`FrameProcessor`]'s own obligations: that
/// [`Self::noise_reduction`] is constant for the lifetime of an instance and validates, that every
/// gain it applies is at most unity so that it never amplifies, that its warm-up is the position
/// count it declared, and that an [`ActivityInput::Optional`] hint is genuinely optional.
///
/// **What it does not promise.** Nothing here bounds how well a reducer works, or on what. Read the
/// implementation's own documentation for what it removes, what it damages while removing it, and
/// when it makes speech worse than leaving it alone — [`SubbandSuppressor`]'s says all three.
pub trait NoiseReducer: FrameProcessor {
    /// Everything this reducer declares beyond its processor capability (§3).
    fn noise_reduction(&self) -> NoiseReduction;
}

impl<R: NoiseReducer + ?Sized> NoiseReducer for Box<R> {
    fn noise_reduction(&self) -> NoiseReduction {
        (**self).noise_reduction()
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
    use crate::dsp::contract::ParameterSpec;

    const FLAGGED: &[ParameterSpec] = &[
        ParameterSpec::new("active", ParameterDomain::Flag),
        ParameterSpec::new("count", ParameterDomain::Integer { min: 0, max: 1 }),
    ];

    /// §3.4: an activity claim is checked against the schema that would have to back it.
    #[test]
    fn an_activity_claim_is_checked_against_the_schema() {
        let capability = DspCapability::new("t").with_parameters(FLAGGED);
        let declaration = NoiseReduction::new(capability);

        assert_eq!(
            declaration
                .with_activity_input(ActivityInput::Optional {
                    parameter: "active"
                })
                .validate(),
            Ok(())
        );
        assert_eq!(
            declaration
                .with_activity_input(ActivityInput::Optional { parameter: "count" })
                .validate(),
            Err(NoiseReductionError::ActivityParameterNotAFlag { parameter: "count" })
        );
        assert_eq!(
            declaration
                .with_activity_input(ActivityInput::Optional {
                    parameter: "missing"
                })
                .validate(),
            Err(NoiseReductionError::ActivityParameterUndeclared {
                parameter: "missing"
            })
        );
    }

    /// §3.2: a host requirement is compared with the caller's inventory and never with the machine.
    #[test]
    fn a_host_requirement_is_compared_with_what_the_caller_declared() {
        let declaration = NoiseReduction::new(DspCapability::new("t"));
        assert_eq!(declaration.admits_host(&[]), Ok(()));

        let needs = declaration.with_host_requirement(HostRequirement::Device { id: "npu" });
        assert_eq!(
            needs.admits_host(&["gpu"]),
            Err(NoiseReductionError::DeviceUnavailable { required: "npu" })
        );
        assert_eq!(needs.admits_host(&["gpu", "npu"]), Ok(()));
        assert_eq!(
            declaration
                .with_host_requirement(HostRequirement::Device { id: "" })
                .validate(),
            Err(NoiseReductionError::EmptyDeviceIdentifier)
        );
    }

    /// §3.1: the warm-up is bounded, and the ceiling is the contract's own.
    #[test]
    fn the_warm_up_is_bounded_by_the_contracts_own_ceiling() {
        let declaration = NoiseReduction::new(DspCapability::new("t"));
        assert_eq!(
            declaration
                .with_warm_up_positions(MAX_WARM_UP_POSITIONS)
                .validate(),
            Ok(())
        );
        assert_eq!(
            declaration
                .with_warm_up_positions(MAX_WARM_UP_POSITIONS + 1)
                .validate(),
            Err(NoiseReductionError::WarmUp {
                positions: MAX_WARM_UP_POSITIONS + 1
            })
        );
    }
}
