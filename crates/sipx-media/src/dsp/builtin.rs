//! The workspace processor registry (`docs/specs/call-dsp-graph.md` §3.2, `M-65`).
//!
//! `docs/specs/custom-call-dsp.md` §7.1 admits the proven-inline profile "only for processors
//! inside this workspace", because "proven" names evidence in this repository's gate and an
//! application cannot add to it. This module is what makes that a door rather than a sentence: a
//! stage carries workspace provenance if and only if it was named through [`BuiltIn`], which is a
//! closed set of this workspace's own processors. Handing the very same processor to
//! [`GraphPlan::with_processor`](super::GraphPlan::with_processor) still refuses it, because what
//! is admitted is the door and not the type.

use sipx_audio::dsp::effects::{
    BitCrush, Gain, HardClip, HighPass, LowPass, Peaking, Polarity, SoftClip, Stutter,
};
use sipx_audio::dsp::{CapabilityError, FrameProcessor};

/// One of this workspace's own processors, selected by name (`docs/specs/call-dsp-effects.md` §2).
///
/// Selecting a variant is the only way a stage reaches a graph under
/// [`ExecutionProfile::ProvenInline`](sipx_audio::dsp::ExecutionProfile::ProvenInline). The
/// processors themselves are ordinary public types in
/// [`sipx_audio::dsp::effects`](sipx_audio::dsp::effects) that anybody may construct and use; what
/// this enum grants is provenance, not access.
///
/// Every variant is configured through the contract's own [`Parameter`](sipx_audio::dsp::Parameter)
/// vocabulary at [`GraphPlan::with_built_in`](super::GraphPlan::with_built_in), so there is no
/// second parameter language beside `M-63`'s. The one thing a parameter cannot express is a
/// processor's *shape*: [`Self::Stutter`]'s line is its declared latency and tail, which a graph
/// sizes buffers from before the first frame, so it is chosen here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BuiltIn {
    /// A saturating amplitude gain, optionally ramped ([`Gain`]).
    Gain,
    /// A polarity inversion ([`Polarity`]).
    Polarity,
    /// A symmetric hard clipper ([`HardClip`]).
    HardClip,
    /// A soft clipper with a quadratic knee ([`SoftClip`]).
    SoftClip,
    /// A bit-depth and sample-rate reducer ([`BitCrush`]).
    BitCrush,
    /// A bounded delay line that can be told to loop what it holds ([`Stutter`]).
    Stutter {
        /// The line, in positions, fixed for the life of the stage. It is declared as both the
        /// processor's latency and its tail, so it is held against the graph's
        /// `retained_tail_positions` bound like any other stage's.
        delay_positions: u32,
    },
    /// A one-pole low-pass ([`LowPass`]).
    LowPass,
    /// A one-pole high-pass ([`HighPass`]).
    HighPass,
    /// A peaking band lift or cut ([`Peaking`]).
    Peaking,
}

impl BuiltIn {
    /// The declared identifier this processor registers under.
    ///
    /// The same string `sipx_audio::dsp::effects::BUILT_IN_IDS` lists and the same one the
    /// processor's own capability declares — checked rather than assumed, because a registry whose
    /// names drifted from the processors' would name something that is not there.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Gain => sipx_audio::dsp::effects::GAIN,
            Self::Polarity => sipx_audio::dsp::effects::POLARITY,
            Self::HardClip => sipx_audio::dsp::effects::HARD_CLIP,
            Self::SoftClip => sipx_audio::dsp::effects::SOFT_CLIP,
            Self::BitCrush => sipx_audio::dsp::effects::BIT_CRUSH,
            Self::Stutter { .. } => sipx_audio::dsp::effects::STUTTER,
            Self::LowPass => sipx_audio::dsp::effects::LOW_PASS,
            Self::HighPass => sipx_audio::dsp::effects::HIGH_PASS,
            Self::Peaking => sipx_audio::dsp::effects::PEAKING,
        }
    }

    /// Build one instance.
    ///
    /// # Errors
    ///
    /// [`CapabilityError`] for a shape the processor does not admit — today only a
    /// [`Self::Stutter`] line past `sipx_audio::dsp::effects::MAX_STUTTER_POSITIONS`. It is
    /// refused here, where the caller wrote it, rather than at validation: a plan that cannot even
    /// be built is not a plan a call has to be told about.
    pub(super) fn processor(self) -> Result<Box<dyn FrameProcessor + Send>, CapabilityError> {
        let processor: Box<dyn FrameProcessor + Send> = match self {
            Self::Gain => Box::new(Gain::new()),
            Self::Polarity => Box::new(Polarity::new()),
            Self::HardClip => Box::new(HardClip::new()),
            Self::SoftClip => Box::new(SoftClip::new()),
            Self::BitCrush => Box::new(BitCrush::new()),
            Self::Stutter { delay_positions } => Box::new(Stutter::new(delay_positions)?),
            Self::LowPass => Box::new(LowPass::new()),
            Self::HighPass => Box::new(HighPass::new()),
            Self::Peaking => Box::new(Peaking::new()),
        };
        Ok(processor)
    }

    /// Every registered processor, in `BUILT_IN_IDS` order.
    ///
    /// [`Self::Stutter`] appears with a one-position line, which is the shortest shape that still
    /// holds audio; this names what exists rather than a configuration to use. It is test-only
    /// because nothing in the crate needs to enumerate the registry — what a caller needs is to
    /// name one variant — and a public list would be surface added for a check rather than for a
    /// user. `M-67` is where an SDK gets a reason to publish one.
    #[cfg(test)]
    const REGISTERED: &'static [Self] = &[
        Self::Gain,
        Self::Polarity,
        Self::HardClip,
        Self::SoftClip,
        Self::BitCrush,
        Self::Stutter { delay_positions: 1 },
        Self::LowPass,
        Self::HighPass,
        Self::Peaking,
    ];
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
    use sipx_audio::dsp::ExecutionProfile;
    use sipx_audio::dsp::effects::{BUILT_IN_IDS, MAX_STUTTER_POSITIONS};

    /// §3.2: the registry names exactly the processors this workspace ships, and each name is the
    /// one the processor itself declares. A registry that drifted from the effects would grant
    /// provenance to something that is not there.
    #[test]
    fn the_registry_is_exactly_what_the_workspace_ships() {
        let registered: Vec<&str> = BuiltIn::REGISTERED.iter().map(|entry| entry.id()).collect();
        assert_eq!(registered, BUILT_IN_IDS);

        for entry in BuiltIn::REGISTERED {
            let processor = entry.processor().expect("a registered shape is admissible");
            let capability = processor.capability();
            assert_eq!(capability.id(), entry.id());
            capability.validate().expect("a registered declaration");
            // Every one of them is the profile this door exists to admit, and no other.
            assert_eq!(
                capability.execution().profile(),
                ExecutionProfile::ProvenInline,
                "{}",
                entry.id()
            );
        }
    }

    /// §3.2: a shape the processor does not admit is refused where it was written.
    #[test]
    fn an_inadmissible_shape_is_refused_at_the_registry() {
        assert!(
            BuiltIn::Stutter {
                delay_positions: MAX_STUTTER_POSITIONS + 1,
            }
            .processor()
            .is_err()
        );
        assert!(
            BuiltIn::Stutter {
                delay_positions: MAX_STUTTER_POSITIONS,
            }
            .processor()
            .is_ok()
        );
    }
}
