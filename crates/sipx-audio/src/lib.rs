//! Telephony audio primitives: G.711 (µ-law and A-law), linear PCM mixing, WAV reading and
//! writing, and Opus behind the `opus` feature.
//!
//! Codecs are pure Rust by default. Opus lives behind the `opus` feature because it binds
//! to C.
//!
//! **G.722 and resampling are absent, and no story is coming for either** (`X-26`). Both were
//! named in this crate's description and in the paragraph above from the commit that scaffolded
//! the workspace, and neither was ever written: no story cut them, no spec asked for them, and
//! nothing else in the stack expects them. The wideband slot G.722 would have filled is Opus's
//! (`M-13`), and the rest of the stack has always agreed — `Codec::from_payload_type(9)` returns
//! `None` and an offer of G.722 alone is refused. Resampling is *deliberately* absent rather
//! than merely missing: `sipx-cli` rejects a clip that is not 8 kHz instead of resampling it
//! quietly, because audio resampled by accident is recognisably wrong rather than obviously
//! broken. Either one is welcome back as a story that argues for it, not as a word in a blurb.
//!
//! RFC 4733 DTMF is not here either, and never was: telephone-events are an RTP payload format
//! rather than audio samples, and they live in `sipx-rtp`.
//!
//! `scripts/check-audio-claims.py` holds the summary above, the package description and the
//! website's crate table to what this crate implements, so the next codec named here has to
//! exist before the gate goes green.

pub mod g711;
pub mod mix;
#[cfg(feature = "opus")]
pub mod opus;
pub mod wav;

pub use g711::{alaw_decode, alaw_encode, ulaw_decode, ulaw_encode};
pub use mix::{mix_excluding, mix_into};
pub use wav::{Wav, read_wav, write_wav};
