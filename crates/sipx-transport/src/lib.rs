//! Async SIP transports.
//!
//! This crate is the driver for the sans-IO core in [`sipx_sip`]: it owns sockets,
//! connections and timers, feeds received bytes in as inputs, and performs the outputs the
//! core asks for. It also implements target resolution (RFC 3263) and connection reuse.
//!
//! Transports are feature-gated so a UDP-only build pulls in no TLS or WebSocket
//! machinery.
