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
//! pacing clock, SRTP keyed from SDES, quality statistics.
//!
//! **Experimental**, and each for the same reason — nothing above this crate selects it:
//!
//! - [`dtls`] — a DTLS-SRTP handshake with no caller. `Config.srtp` takes `SrtpKeys` and the handshake
//!   produces `srtp::Context`, so the two do not currently meet (`M-28`).
//! - [`ice`] — a complete agent that no call gathers with (`M-27`).
//! - [`Bridge`] and [`Conference`] — real and tested over sessions **you** own; a `Call` does not hand
//!   out its `MediaSession`, so two calls cannot be bridged yet (`C-6`).

pub mod bridge;
pub mod conference;
pub mod dtls;
pub mod ice;
pub mod session;

pub use bridge::Bridge;
pub use conference::Conference;
pub use dtls::{Arriving, Handshake, Profile, Role};
pub use session::{
    Codec, Config, Encoded, Interrupt, MediaPort, MediaSession, Playback, PlaybackEnd, PlaybackId,
    SrtpKeys,
};
