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

pub mod session;

pub use session::{Codec, Config, MediaPort, MediaSession};
