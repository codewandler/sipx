//! The custom call-DSP contract: the one deterministic processor interface built-in effects,
//! noise reduction and application-supplied DSP all implement (`M-63`).
//!
//! The normative contract is `docs/specs/custom-call-dsp.md`; where this module and that document
//! disagree, the document is right until it is changed deliberately. Section numbers in the
//! documentation here refer to it.
//!
//! [`FrameProcessor`] is a synchronous frame transform and a pure state machine. Its whole input
//! vocabulary is a validated [`DspCapability`], a finite [`Parameter`] set, a declared
//! [`StreamFormat`] and direction, [`DspFrame`]s carrying explicit metadata, and requested resets,
//! flushes and cancellations; its whole output vocabulary is samples written into a caller-owned
//! [`FrameSink`], typed [`DspObservation`]s and typed errors. It owns no socket, no device, no
//! device enumeration, no clock, no thread and no task, and it allocates none of the memory it
//! works in — the output, the observation queue and the [`Scratch`] region are all lent by the
//! caller. Time is the sample position and rate it is handed, which is why the same vectors hold
//! under virtual and wall time alike.
//!
//! It reuses rather than restates: [`crate::analysis::AudioDirection`] and
//! [`crate::analysis::DiscontinuityKind`] are the analysis contract's own types, the rate domain
//! and its [`crate::PcmError`] refusal are the linear-PCM boundary's, and the observation queue's
//! coalescing loss accounting is the analysis contract's §8.3 applied unchanged. Two observers of
//! one call that name its sides or its breaks differently is how a disagreement about what
//! happened starts.
//!
//! Attaching a processor to live call audio is not this module's job: that is `M-54`'s bounded PCM
//! seam and `M-64`'s call-local graph, and `docs/specs/custom-call-dsp.md` §10 forbids a second
//! call-media tap by name.
//!
//! # What implements it here
//!
//! [`effects`] is `M-65`'s nine deterministic effects and filters, and [`noise`] is `M-66`'s
//! interchangeable noise reduction: a declaration and a trait with one shipped implementation
//! behind them, plus a second written from outside this crate so that "a different one can be
//! substituted" is a fact rather than a shape. Both reach the contract through this module's public
//! surface and nothing else. Noise reduction is **not** voice activity detection, echo
//! cancellation, recognition or automatic gain control, and [`noise`] says so at length and says
//! what its baseline damages while it works.
//!
//! # Execution profiles
//!
//! [`ExecutionProfile`] is the part of this contract that is a promise to an application about its
//! call quality, so read what each variant does **and does not** claim. Exactly two of the three —
//! [`ExecutionProfile::ProvenInline`] and [`ExecutionProfile::SupervisedIsolated`] — may claim that
//! over-budget work cannot stall RTP, by two different mechanisms
//! ([`OverrunContainment::BoundedWork`] and [`OverrunContainment::BoundedWait`]).
//! [`ExecutionProfile::TrustedCooperativeNative`] is trusted application code on the media worker
//! and claims nothing about containment: sipx cannot preempt it, cancel it or reap it, and a
//! callback that does not return stalls RTP for that call. [`ExecutionProfile::contains_overrun`]
//! reports that distinction, it is not configurable, and no conformance result changes it.
//!
//! # Conformance
//!
//! [`conformance`] holds the harness. It accepts any processor from any crate through a factory,
//! reports every check it declares — including the ones it could not prove, by name, as
//! [`conformance::CheckStatus::Unproven`] rather than as passes — and is itself proved by fixtures
//! that violate each invariant on purpose.
//!
//! One check, `DSP-K9`, needs something this workspace cannot contain. Holding a processor's heap
//! to its declared `state_bytes` means counting allocations, counting allocations means
//! `unsafe impl GlobalAlloc`, and `unsafe_code` is `forbid`den here — for tests as much as for
//! libraries, and a `forbid` is not something an `allow` can reopen. So [`Conformance::run`] reports
//! that half `Unproven` and says where the proof is, while [`Conformance::run_with_heap_meter`]
//! takes a [`HeapMeter`] from a caller that has one and reports a measured figure. `heap-probe/`,
//! outside the workspace, is the caller that has one.
//!
//! ```
//! use sipx_audio::analysis::AudioDirection;
//! use sipx_audio::dsp::{
//!     Conformance, DspCapability, DspFrame, DspResetCause, FrameAdmission, FrameProcessor,
//!     FrameSink, Parameter, ParameterError, ProcessError, Scratch, StreamFormat,
//! };
//!
//! /// The identity processor of §12.1.
//! #[derive(Default)]
//! struct Ident {
//!     admission: FrameAdmission,
//! }
//!
//! impl FrameProcessor for Ident {
//!     fn capability(&self) -> DspCapability {
//!         DspCapability::new("ident")
//!     }
//!     fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
//!         self.capability().validate_parameters(parameters)
//!     }
//!     fn prepare(
//!         &mut self,
//!         direction: AudioDirection,
//!         format: StreamFormat,
//!     ) -> Result<(), sipx_audio::dsp::FormatError> {
//!         self.admission.prepare(&self.capability(), direction, format)
//!     }
//!     fn process(
//!         &mut self,
//!         frame: &DspFrame<'_>,
//!         _scratch: &mut Scratch<'_>,
//!         sink: &mut FrameSink<'_>,
//!     ) -> Result<(), ProcessError> {
//!         self.admission.admit(&self.capability(), frame)?;
//!         sink.write(frame.samples())
//!     }
//!     fn flush(&mut self, _sink: &mut FrameSink<'_>) -> Result<(), ProcessError> {
//!         self.admission.admit_flush()
//!     }
//!     fn reset(&mut self, _cause: DspResetCause) {
//!         self.admission.reset();
//!     }
//!     fn cancel(&mut self) {
//!         self.admission.cancel();
//!     }
//!     fn retained(&self) -> u32 {
//!         0
//!     }
//! }
//!
//! let report = Conformance::new().run(Ident::default);
//! assert!(report.passed(), "{report}");
//! ```

pub mod conformance;
mod contract;
pub mod effects;
pub mod noise;

pub use conformance::{
    CHECK_ALLOCATION, CHECK_CANCELLATION, CHECK_CAPABILITY, CHECK_CHUNK_BOUNDARY,
    CHECK_DETERMINISM, CHECK_DISCONTINUITY, CHECK_EXTREMES, CHECK_FORMAT_REFUSAL,
    CHECK_FRAME_REFUSAL, CHECK_LENGTH, CHECK_PARAMETER_REFUSAL, CHECK_RESET, CHECKS, CheckOutcome,
    CheckStatus, Conformance, ConformanceReport, HeapMeter, HeapUse,
};
pub use contract::{
    Admitted, CapabilityError, DeadlineAction, DspCapability, DspFrame, DspObservation,
    DspResetCause, ExecutionPolicy, ExecutionProfile, FailureAction, FormatError, FrameAdmission,
    FrameProcessor, FrameSink, LengthPolicy, MAX_CHANNELS, MAX_CONSECUTIVE_MISSES,
    MAX_DEADLINE_FRAMES, MAX_FRAME_SAMPLES, MAX_LATENCY_POSITIONS, MAX_PARAMETERS,
    MAX_SCRATCH_SAMPLES, OverrunContainment, Parameter, ParameterDomain, ParameterError,
    ParameterSpec, ParameterValue, ProcessError, RateSupport, ResetBehavior, Scratch, StreamFormat,
};
