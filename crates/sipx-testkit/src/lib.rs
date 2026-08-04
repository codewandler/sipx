//! Shared test machinery for the sipx workspace.
//!
//! Provides a loopback transport that lets two full stacks talk inside one process with no
//! sockets, the RFC 4475 and RFC 5118 torture-message corpora and their harnesses, a private
//! certificate authority for the TLS tests, and fixtures for interoperability runs against
//! third-party servers.
//!
//! Not published to crates.io.

pub mod certs;
pub mod link;
pub mod rfc4475;
pub mod rfc5118;
pub mod soak;
pub mod transaction_sequence;
