//! RTP and RTCP (RFC 3550): packet encoding and decoding, sequence handling, jitter
//! buffering, and sender/receiver report statistics.
//!
//! Like the SIP core, packet handling here is sans-IO — the socket layer lives in
//! `sipx-media`.
