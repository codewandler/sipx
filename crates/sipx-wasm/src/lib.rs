//! The sans-I/O browser session kernel: `sipx.browser.v1`.
//!
//! This crate is the Rust half of the browser SDK. It compiles the selected SIP, SDP, transaction
//! and dialog state into a state machine whose entire environment is explicit: the host supplies
//! bytes, fired timers, monotonic time and cryptographic entropy, and receives outbound bytes,
//! timer requests and typed events. Nothing here opens a socket, reads a clock, spawns a task,
//! touches a filesystem or asks for randomness.
//!
//! [`docs/specs/browser-sdk.md`](../../../docs/specs/browser-sdk.md) is normative. Where this
//! crate and that document disagree, the document is right. In particular:
//!
//! - **§3.1** — the kernel owns parsing, transactions, dialogs, registration, digest
//!   authentication and SDP *policy*. It has no WebAssembly imports at all, so it can never call
//!   the host and reentrancy is structurally impossible.
//! - **§3.2, §3.3** — the browser owns WSS, `RTCPeerConnection`, ICE, DTLS-SRTP, capture and
//!   render. Video, data channels, SCTP and a Rust WebRTC engine are refused at the contract
//!   level, not merely absent.
//! - **§4.7, §8.4** — entropy is a host input with a deterministic derivation tape and **no
//!   fallback**. There is no time seed, no counter seed and no weaker generator in this crate or
//!   in anything it depends on; the browser build drops `sipx-sip`'s `identity` and `sipx-sdp`'s
//!   `sdes-keys` features precisely so that no operating-system entropy source is reachable.
//!
//! # Shape
//!
//! [`Abi`] is the §4.3 export surface, one method per export, in safe Rust. The WebAssembly
//! module is a thin `extern "C"` shim over it, which is what makes §9's vectors runnable
//! unchanged against native Rust and against the compiled module.
//!
//! ```no_run
//! use sipx_wasm::Abi;
//!
//! let mut abi = Abi::new();
//! let config = br#"{"v":1,"aor":"sip:alice@example.net","auth":{"username":"alice","password":"secret"},"transport":{"scheme":"wss","host":"edge.example.net","resource":"/sip"},"insecure":"refuse"}"#;
//! let ptr = abi.alloc_with(config);
//! let handle = abi.kernel_new(ptr, config.len() as u32);
//! abi.free(ptr, config.len() as u32);
//! assert!(handle > 0);
//!
//! // After every state-advancing entry point, drain until `0` (§4.6).
//! while abi.next_output(handle) != 0 {
//!     let _record = abi.borrowed(handle);
//! }
//! ```
//!
//! # Stability
//!
//! **Experimental.** The epic that owns this contract is not finished, the npm package it ships
//! inside is pre-1.0, and completing the epic does not imply a stable 1.0 API (§7.4). What *is*
//! stable within `sipx.browser.v1` is the ABI: additive fields, additive event types and appended
//! error codes are compatible changes, and anything that breaks a §9 vector requires
//! `sipx.browser.v2` and an [`abi::ABI_VERSION`] bump.

// This crate's inline test modules opt out of coverage instrumentation, so the
// published figure measures the code rather than the tests measuring it. Never set outside
// `cargo llvm-cov`, so every other build parses this and discards it. Applied by
// `./scripts/coverage-report.py --annotate`; `docs/coverage.md` states what it costs.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

pub mod abi;
mod bounds;
mod command;
mod config;
mod entropy;
pub mod error;
mod event;
mod json;
mod kernel;
mod output;
mod sip;

pub use abi::{ABI_VERSION, Abi, unpack};
pub use error::Error;
pub use output::Record;

/// The §4.9 bounds, as public constants.
///
/// Exported because they are contract values a host has to agree with — the glue sizes its input
/// buffers from them and `A-17`'s drift test holds the generated package to the same numbers.
pub mod limits {
    pub use crate::bounds::{
        ENTROPY_CAPACITY, ENTROPY_LOW_WATER, MAX_CALLS, MAX_COMMAND, MAX_EVENT, MAX_HANDLES,
        MAX_LINEAR_MEMORY, MAX_PENDING_TIMERS, MAX_QUEUED_BYTES, MAX_QUEUED_RECORDS, MAX_SDP,
        MAX_SIP_MESSAGE,
    };
}
