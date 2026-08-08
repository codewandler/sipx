//! Media sessions: RTP over UDP, paced sending, buffered receiving.
//!
//! Two decisions shape this crate.
//!
//! **Symmetric RTP.** Media goes back to where it arrives from, not to the address the SDP
//! advertised. Behind a NAT the advertised address is private and the only path back is the
//! pinhole the far end opened by sending.
//!
//! **The clock lives in one place.** Audio is paced by one interval timer at the packetisation
//! interval. Sending on channel readiness instead makes the packet rate depend on how fast the
//! application produces samples, which is how a call sends 200 packets per second to a jitter
//! buffer expecting 50.
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
//! **Supported**: `MediaSession` and the RTP/RTCP plumbing under it — binding, symmetric RTP, the
//! pacing clock, SRTP keyed from SDES, quality statistics — plus [`ice`], whose gathering and
//! selected-pair driver are consumed by `sipx-call`, and [`dtls`]'s protocol, key-derivation and
//! handshake surface, which `sipx-call` selects for explicit DTLS-SRTP policy (`M-28`). [`processing`]
//! is the one call-audio tap, specified in `docs/specs/call-audio-seam.md` (`M-54`); local speech
//! and deterministic call-audio analysis both ride it rather than adding a second.
//!
//! **Experimental**:
//!
//! - `dtls::openssl` — the optional OpenSSL implementation behind the off-by-default `dtls`
//!   feature. No shipped application enables it by default; the feature never changes a session or
//!   call without explicit DTLS-SRTP policy.
//! - [`Bridge`] and [`Conference`] — real and tested over sessions **you** own; a `Call` does not hand
//!   out its `MediaSession`, so two calls cannot be bridged yet (`C-6`).

pub mod bridge;
pub mod browser;
pub mod conference;
mod counters;
pub mod dtls;
pub mod ice;
pub mod processing;
pub mod session;

pub use bridge::Bridge;
pub use conference::{Conference, ConferenceError};
pub use counters::MediaDiscardCounts;
pub use dtls::{Arriving, Handshake, Profile, Role};
pub use ice::IcePath;
pub use processing::{
    AudioDirection, Discontinuity, DiscontinuityKind, PcmFrame, PcmProcessor, Processing,
    ProcessingError,
};
#[cfg(feature = "dtls")]
pub use session::DtlsStartError;
pub use session::{
    Codec, CodecDirection, Config, Encoded, Interrupt, MediaPort, MediaSession, PcmCapture,
    Playback, PlaybackEnd, PlaybackId, RtcpQualityHook, RtcpQualitySample, SetupError, SrtpKeys,
    StartError,
};
pub use sipx_audio::{Pcm, PcmEncoding, PcmError, PcmFormat, PcmSamples};
