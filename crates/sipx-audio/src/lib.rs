//! Telephony audio primitives: G.711 (µ-law and A-law), G.722, linear PCM mixing and
//! resampling, WAV reading and writing, and RFC 4733 DTMF events.
//!
//! Codecs are pure Rust by default. Opus lives behind the `opus` feature because it binds
//! to C.
