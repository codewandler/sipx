//! Shared test machinery for the sipx workspace.
//!
//! Provides an application-call harness with in-process SIP signalling, a seeded virtual-time
//! transaction link, the RFC 4475 and RFC 5118 torture-message corpora and their harnesses, and a
//! private certificate authority for TLS tests.
//!
//! Published so downstream applications can use the same deterministic call surface as the
//! workspace. The corpus and certificate modules are support utilities; [`call`] is the small,
//! supported downstream call and RTP-echo harnesses.
//!
//! # Stability
//!
//! sipx is pre-1.0. **Supported** APIs are meant to be depended on and receive migration guidance
//! for breaking changes; new enum variants and fields may still appear before 1.0. **Experimental**
//! APIs may change shape without a migration note.
//!
//! **Supported:** [`call`], [`rtp_echo`], [`link`] and [`time`] — the real application-call harness
//! over socket-free SIP signalling, bounded RTP media peer, seeded fault link and nanosecond virtual
//! clock. **Experimental:** certificate, corpus, soak and transaction-sequence utilities; these
//! primarily serve workspace verification and their shape follows those suites.

pub mod call;
pub mod certs;
pub mod link;
pub mod rfc4475;
pub mod rfc5118;
pub mod rtp_echo;
pub mod soak;
pub mod time;
pub mod transaction_sequence;
