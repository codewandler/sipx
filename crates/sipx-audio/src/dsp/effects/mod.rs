//! The processors this workspace ships: `docs/specs/call-dsp-effects.md` (`M-65`).
//!
//! Nine deterministic effects and filters, every one of them a plain
//! [`FrameProcessor`](crate::dsp::FrameProcessor) built from the public contract in
//! [`crate::dsp`] and from nothing else. There is no crate-private door: what a built-in gets is
//! what an application gets, which is the only way `docs/specs/custom-call-dsp.md` §10's claim that
//! built-in and application-supplied processors behave identically is a fact rather than an
//! intention. The conformance harness runs against these through the same factory it runs against
//! anybody's.
//!
//! # What every one of them promises
//!
//! Each is sans-I/O, allocation-free after construction, and a pure function of its capability, its
//! parameters, its declared format and direction and the frames it was given — so the same vectors
//! hold on every architecture and under virtual or wall time alike. Arithmetic is integer at stated
//! widths and saturates rather than wrapping; there is no floating point anywhere and no
//! transcendental function even in coefficient derivation, which is why the filters ship a table
//! (`docs/specs/custom-call-dsp.md` §4.6). Each declares
//! [`ExecutionProfile::ProvenInline`](crate::dsp::ExecutionProfile::ProvenInline) and
//! [`LengthPolicy::Preserving`](crate::dsp::LengthPolicy::Preserving): bounded per-frame work on
//! the media worker, and one position out for one position in, which is what
//! `docs/specs/call-dsp-graph.md` §4.3 requires of anything attached to a live call.
//!
//! Each accepts **mono and stereo** and every rate the linear-PCM boundary admits. The contract's
//! channel ceiling is eight; these declare two, because two is what the call paths this epic serves
//! carry and what this workspace measures. A declaration is what a caller sizes buffers from rather
//! than a place to be generous.
//!
//! # What none of them promises
//!
//! None of them is adaptive: there is no detector, no envelope follower, no automatic level and no
//! noise estimate anywhere in this module — noise reduction is `M-66` and is deliberately not here.
//! None of them resamples, changes a frame's position count, or looks at anything but the samples
//! it was handed. None reads a clock: `smoothing_positions`, `hold_positions` and a delay line are
//! all counted in **positions**, so an effect behaves the same on a 20 ms and a 60 ms packet.
//!
//! # Smoothing, and how it is discovered
//!
//! A processor that ramps a parameter change declares `smoothing_positions` with its closed range;
//! a processor that applies a change whole declares no such parameter. Smoothing is therefore
//! published exactly the way every other property is — through the capability's own parameter
//! schema — and the absence of the parameter is as discoverable as its presence. Today
//! [`Gain`] is the one processor with a value continuous enough for a ramp to mean anything; every
//! other built-in states in its own documentation why a change of its parameters is applied whole.
//!
//! # Intentional glitches are not defects
//!
//! [`Stutter`] is an effect and says so in a vocabulary nothing else uses: a requested repeat is a
//! [`ParameterApplied`](crate::dsp::DspObservation::ParameterApplied) observation naming the
//! parameter and the position. A break in the call's timeline is a
//! [`Restarted`](crate::dsp::DspObservation::Restarted); a deadline miss is a graph transition no
//! processor can emit at all. Neither this module nor its documentation presents an overload defect
//! as an effect, and the three facts stay separable in events and metrics because they are three
//! types in two different streams.

mod arithmetic;
mod filter;
mod frame;
mod glitch;
mod level;
mod shape;

pub use filter::{HighPass, LowPass, Peaking};
pub use glitch::{MAX_STUTTER_POSITIONS, Stutter};
pub use level::{Gain, Polarity};
pub use shape::{BitCrush, HardClip, SoftClip};

/// The arithmetic, frame plumbing and one-pole section `M-66`'s noise reduction shares with these.
///
/// Not public: these are `crate::dsp`'s internals, visible to [`crate::dsp::noise`] and nowhere
/// else. A suppressor that admitted frames through its own copy of the refusal taxonomy, or that
/// re-derived the one-pole coefficient from its own table, would be a second reading of documents
/// whose whole point is that there is one — so the reuse is the design and not a convenience.
pub(in crate::dsp) use filter::{OnePole, coefficient};
pub(in crate::dsp) use frame::{Body, CHANNELS, MAX_CHANNELS, open, state_bytes, transform};

pub(in crate::dsp) use arithmetic::{narrow, scaled};

/// [`Gain`]'s declared identifier.
pub const GAIN: &str = "sipx.gain";
/// [`Polarity`]'s declared identifier.
pub const POLARITY: &str = "sipx.polarity";
/// [`HardClip`]'s declared identifier.
pub const HARD_CLIP: &str = "sipx.hard_clip";
/// [`SoftClip`]'s declared identifier.
pub const SOFT_CLIP: &str = "sipx.soft_clip";
/// [`BitCrush`]'s declared identifier.
pub const BIT_CRUSH: &str = "sipx.bit_crush";
/// [`Stutter`]'s declared identifier.
pub const STUTTER: &str = "sipx.stutter";
/// [`LowPass`]'s declared identifier.
pub const LOW_PASS: &str = "sipx.low_pass";
/// [`HighPass`]'s declared identifier.
pub const HIGH_PASS: &str = "sipx.high_pass";
/// [`Peaking`]'s declared identifier.
pub const PEAKING: &str = "sipx.peaking";

/// Every processor this workspace ships, by declared identifier, in a stable order.
///
/// This is the list a registry is built from and the one `M-67` will expose to an SDK. It is part
/// of the contract in the way `CHECKS` is: a caller asserting on one of these strings is asserting
/// on a name, so the names do not move. Constructing one is done through its own type — there is no
/// string-keyed factory here, because a processor whose shape depends on a construction argument
/// ([`Stutter`]'s line, which is its declared latency and tail) cannot be built from an identifier
/// alone, and a factory that quietly picked a default would be choosing an application's latency
/// for it.
pub const BUILT_IN_IDS: &[&str] = &[
    GAIN, POLARITY, HARD_CLIP, SOFT_CLIP, BIT_CRUSH, STUTTER, LOW_PASS, HIGH_PASS, PEAKING,
];
