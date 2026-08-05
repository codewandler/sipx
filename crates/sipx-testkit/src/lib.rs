//! Shared test machinery for the sipx workspace.
//!
//! Provides a loopback transport that lets two full stacks talk inside one process with no
//! sockets, the RFC 4475 and RFC 5118 torture-message corpora and their harnesses, a private
//! certificate authority for the TLS tests, and fixtures for interoperability runs against
//! third-party servers.
//!
//! Published so downstream applications can use the same deterministic call surface as the
//! workspace. The corpus and certificate modules are support utilities; [`call`] is the small,
//! supported downstream call harness.
//!
//! # Stability
//!
//! sipx is pre-1.0. **Supported** APIs are meant to be depended on and receive migration guidance
//! for breaking changes; new enum variants and fields may still appear before 1.0. **Experimental**
//! APIs may change shape without a migration note.
//!
//! **Supported:** [`call`], [`link`] and [`time`] — the socket-free call harness, seeded fault link
//! and explicit virtual clock. **Experimental:** certificate, corpus, soak and transaction-sequence
//! utilities; these primarily serve workspace verification and their shape follows those suites.

pub mod call;
pub mod certs;
pub mod link;
pub mod rfc4475;
pub mod rfc5118;
pub mod soak;
pub mod time;
pub mod transaction_sequence;
