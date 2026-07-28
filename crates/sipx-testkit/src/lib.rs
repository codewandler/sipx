//! Shared test machinery for the sipx workspace.
//!
//! Provides a loopback transport that lets two full stacks talk inside one process with no
//! sockets, the RFC 4475 torture-message corpus and its harness, a private certificate
//! authority for the TLS tests, and fixtures for interoperability runs against third-party
//! servers.
//!
//! Not published to crates.io.

pub mod certs;
pub mod link;
pub mod load;
pub mod rfc4475;
pub mod soak;
