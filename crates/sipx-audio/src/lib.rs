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
//! RFC 4733 DTMF is not here either, and never was: telephone-events are an RTP payload format
//! rather than audio samples, and they live in `sipx-rtp`.
//!
//! `scripts/check-audio-claims.py` holds the summary above, the package description and the
//! website's crate table to what this crate implements, so the next codec named here has to
//! exist before the gate goes green.
//!
//! # Stability
//!
//! sipx is pre-1.0, so **neither word below means frozen**. `1.0.0` is what freezes an API, and its
//! predicates are in `docs/roadmap.md`. Until then:
//!
//! - **Supported** — meant to be depended on. Breaking changes get a `CHANGELOG.md` entry saying what
//!   to do instead. New enum variants and new struct fields may still appear in a minor release, so a
//!   downstream `match` should carry a `_` arm.
//! - **Experimental** — may change shape or be removed without a migration note. Depend on it only if
//!   you are prepared to follow it.
//!
//!
//! **Supported.** G.711, G.722 and L16 both ways, linear PCM conversion/resampling, mixing and WAV. Opus is behind the off-by-default `opus` feature
//! and remains **Experimental** at this crate boundary. `sipx-call` and an Opus-enabled diagnostic
//! CLI can select it, but no default shipped application enables its native dependency. Cargo's
//! normalized package manifests are checked with the feature off and on, including a clean packaged
//! CLI build and run. `M-39` supplies rate-correct bidirectional CLI audio and independent-peer
//! evidence in both SIP roles. Optional RFC 7587 `fmtp` controls are not implemented.

pub mod analysis;
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
