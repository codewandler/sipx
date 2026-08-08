//! Shared test machinery for the sipx workspace.
//!
//! Provides an application-call harness with in-process SIP signalling, a seeded virtual-time
//! transaction link, bounded RTP and realtime-peer fixtures, the RFC 4475 and RFC 5118
//! torture-message corpora and their harnesses, and a private certificate authority for TLS tests.
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
//! clock. **Experimental:** [`realtime_peer`], certificate, corpus, soak and transaction-sequence
//! utilities; these primarily serve workspace verification and their shape follows those suites.
//!
//! This is a test-product surface, not production application reachability. Its package metadata
//! names the independently compiled `rtp_echo` example as the executable caller; the app-surface
//! gate validates that target and import without admitting this crate's dependencies into the
//! production surface.

// This crate's inline test modules opt out of coverage instrumentation, so the
// published figure measures the code rather than the tests measuring it. Never set outside
// `cargo llvm-cov`, so every other build parses this and discards it. Applied by
// `./scripts/coverage-report.py --annotate`; `docs/coverage.md` states what it costs.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

pub mod call;
pub mod certs;
pub mod link;
pub mod realtime_peer;
pub mod rfc4475;
pub mod rfc5118;
pub mod rtp_echo;
pub mod soak;
pub mod time;
pub mod transaction_sequence;
