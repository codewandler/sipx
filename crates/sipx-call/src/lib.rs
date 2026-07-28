//! Call framework — the layer applications build on.
//!
//! A `Call` owns its signalling dialog and its media pipeline outright. Everything above
//! is expressed in those terms: answer and dial, play audio, record, read and send DTMF,
//! bridge two calls, mix several, and transfer (RFC 3515).
//!
//! Bridging moves audio frames between calls over channels rather than sharing a media
//! session behind a lock, so a stalled leg can never block its peer.
