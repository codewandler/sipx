//! Telephony audio primitives: G.711 (µ-law and A-law), G.722, linear PCM mixing and
//! resampling, WAV reading and writing, and RFC 4733 DTMF events.
//!
//! Codecs are pure Rust by default. Opus lives behind the `opus` feature because it binds
//! to C.

pub mod g711;
pub mod mix;
#[cfg(feature = "opus")]
pub mod opus;
pub mod wav;

pub use g711::{alaw_decode, alaw_encode, ulaw_decode, ulaw_encode};
pub use mix::{mix_excluding, mix_into};
pub use wav::{Wav, read_wav, write_wav};
