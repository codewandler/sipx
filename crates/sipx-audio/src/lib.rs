//! Telephony audio primitives: G.711 (µ-law and A-law), G.722, L16, linear PCM conversion and
//! resampling, WAV reading and writing, and Opus behind the `opus` feature.
//!
//! Codecs are pure Rust by default. Opus lives behind the `opus` feature because it binds
//! to C.
//!
//! **G.722 is implemented natively** (`M-44`), reversing the absence `X-26` recorded. The
//! history matters here: this crate's description promised G.722 from the commit that
//! scaffolded the workspace while nothing implemented it, `X-26` removed the claim rather than
//! backfilling the code, and `M-44` implemented the codec from the ITU-T G.722 recommendation —
//! verified against the recommendation's own Appendix II digital test sequences rather than by
//! round-tripping (`docs/specs/g722.md`). The claim exists again because the code does.
//!
//! **Resampling is now explicit and supported** (`M-43`). [`PcmFormat`] names unsigned 8-bit or
//! signed 16-bit mono PCM and a rate from 1 through 384,000 Hz; [`LinearResampler`] converts that
//! stream to another stated rate without guessing either fact from a buffer. The diagnostic CLI
//! uses this same boundary for WAV and device audio rather than maintaining a private resampler.
//!
//! **Deterministic call-audio analysis lives in [`analysis`]** (`M-57`, `M-58`). It is a sans-I/O
//! state machine over borrowed PCM frames — no socket, no device, no clock read, no task, and no
//! speech model: voice activity there is an integer variance predicate over a fixed window, not
//! recognition. Live frames reach it through `sipx-media`'s one bounded call seam, never through a
//! tap of its own. `docs/specs/call-audio-processing.md` is the contract it implements.
//!
//! **The custom call-DSP contract lives in [`dsp`]** (`M-63`). It is the one deterministic frame
//! transform that built-in effects, noise reduction and application-supplied processors all
//! implement — same trait, same capability declaration, same conformance harness, no crate-private
//! door. Like [`analysis`] it is sans-I/O and allocation-free: output, observations and scratch are
//! lent by the caller, and time is the sample position and rate given as input. What it adds is a
//! declaration of *where* a processor may run — see `ExecutionProfile`, which states for each of
//! the three profiles what it does and does not promise about stalling RTP.
//! `docs/specs/custom-call-dsp.md` is the contract it implements.
//!
//! **The processors that run under that contract live in [`dsp::effects`]** (`M-65`): a gain, a
//! polarity inversion, a hard and a soft clipper, a bit crusher, a bounded delay/stutter line, and
//! one-pole low-pass, high-pass and peaking filters. Every one of them is integer arithmetic that
//! saturates rather than wrapping, with no floating point and no transcendental function even in
//! coefficient derivation — the filters ship a table instead — so their vectors are exact on every
//! machine. `docs/specs/call-dsp-effects.md` states each one's arithmetic to the last bit.
//!
//! What that module deliberately is **not** is as load-bearing as what it is. Nothing there is
//! adaptive: there is no detector, no envelope follower, no automatic level and no noise estimate,
//! and noise reduction is a separate contract (`M-66`) rather than a setting here. Nothing there
//! resamples, changes a frame's length or reads a clock — every duration is counted in sample
//! positions. And the stutter is an *effect*: a requested repeat is reported as a parameter that was
//! applied, a break in the call's timeline as a restart, and a late stage as a graph transition no
//! processor can emit — three facts in three types, so that an overload defect is never presented
//! as an effect anywhere in this stack.
//!
//! RFC 4733 DTMF is not here either, and never was: telephone-events are an RTP payload format
//! rather than audio samples, and they live in `sipx-rtp`.
//!
//! `scripts/check-audio-claims.py` holds the summary above, the package description and the
//! website's crate table to what this crate implements, so the next codec named here has to
//! exist before the gate goes green.
//!
//! # Stability
//!
//! The Supported Rust API is frozen for compatible v1 evolution. Existing Supported paths and
//! signatures remain source-compatible throughout major version 1; compatible additions use the
//! reservations documented on their types.
//!
//! - **Supported** — covered by the v1 compatibility contract. A breaking change waits for the next
//!   major version.
//! - **Experimental** — remains unfrozen and may change shape or be removed without a migration
//!   note. Depend on it only if you are prepared to follow it.
//!
//!
//! **Supported.** G.711, G.722 and L16 both ways, linear PCM conversion/resampling, mixing and WAV. Opus is behind the off-by-default `opus` feature
//! and remains **Experimental** at this crate boundary. `sipx-call` and an Opus-enabled diagnostic
//! CLI can select it, but no default shipped application enables its native dependency. Cargo's
//! normalized package manifests are checked with the feature off and on, including a clean packaged
//! CLI build and run. `M-39` supplies rate-correct bidirectional CLI audio and independent-peer
//! evidence in both SIP roles. Optional RFC 7587 `fmtp` controls are not implemented.
//!
//! <!-- BEGIN sipx-api-classification -->
//! **Experimental Rust API roots:**
//!
//! - [`sipx_audio::opus`](crate::opus)
//! <!-- END sipx-api-classification -->

// This crate's inline test modules opt out of coverage instrumentation, so the
// published figure measures the code rather than the tests measuring it. Never set outside
// `cargo llvm-cov`, so every other build parses this and discards it. Applied by
// `./scripts/coverage-report.py --annotate`; `docs/coverage.md` states what it costs.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

pub mod analysis;
pub mod dsp;
pub mod g711;
pub mod g722;
pub mod l16;
pub mod mix;
#[cfg(feature = "opus")]
pub mod opus;
pub mod pcm;
pub mod signal;
pub mod wav;

pub use g711::{alaw_decode, alaw_encode, ulaw_decode, ulaw_encode};
pub use mix::{mix_excluding, mix_into};
pub use pcm::{LinearResampler, Pcm, PcmEncoding, PcmError, PcmFormat, PcmSamples, resample_i16};
pub use wav::{Wav, read_wav, write_wav};
