//! SIP application host with webhook, full-duplex session, and realtime audio bindings.
//!
//! Calls terminated on the sipx stack are driven by customer code over the `sipx.app.v1` contract.
//! [`host`] and [`webhook`] implement document mode end to end, while [`session`] implements the
//! authenticated full-duplex binding and granted call origination. An embedded runtime and packaged
//! TypeScript SDK are not implemented. Configuration and the deterministic harness live beside the
//! host rather than inside the contract interpreter.
//!
//! The contract is specified in `docs/specs/app-contract.md`, the host's design in
//! `docs/designs/app-host.md`, and the work is tracked by the `A-*` stories on the board.
//!
//! **The [`harness`]** (story `A-7`) is the deterministic apparatus every behaviour claim is held to.
//! It runs the contract's own vector set with fake time, a scripted app and scripted call events,
//! which is possible before the call-framework stories land and is the reason it came first. The
//! bindings (`A-2`, `A-4`) are built against it rather than beside it. The harness is a virtual-time
//! driver over `sipx-app-protocol::Interpreter`, the same interpreter production document mode
//! drives.
//!
//! Beside it is [`config`] (story `A-1`): the document that declares a host — its listeners, its
//! apps, what each app is granted, and what a slow, wrong or absent app does to a live call. The
//! two meet where they should: a failure policy read out of a document is the same value the
//! harness runs a scenario with, so a knob nobody consults is not something this crate can express.
//!
//! # Stability
//!
//! The Supported Rust API is frozen for compatible v1 evolution. Existing Supported paths and
//! signatures remain source-compatible throughout major version 1; compatible additions use the
//! reservations documented on their types.
//!
//! - **Supported** — covered by the v1 compatibility contract. A breaking change waits for the next
//!   major version.
//! - **Experimental** — remains unfrozen and may change shape or be removed without a migration
//!   note. Depend on it only if you are prepared to follow it.
//!
//! An experimental item **graduates** when something outside this repository depends on it: a second
//! caller is what constrains a shape, so the answer to "somebody is using it" is a `CHANGELOG.md`
//! entry moving it to *Supported*, not a note asking them to stop. Without that clause this line
//! would be a freeze rather than a measurement (`X-38`).
//!
//! **Supported:** the document-mode path through [`host`] and [`webhook`], the authenticated
//! full-duplex path through [`session`], the G.711 realtime path through [`realtime`] and [`wss`],
//! and the configuration they consume. A webhook or pinned session in another process can drive a
//! real call through `sipx.app.v1`; a realtime binding terminates one call directly. Changes to
//! these host-facing Rust APIs remain source-compatible throughout the v1 line. The `sipx.app.v1`
//! wire name remains Experimental under `sipx-app-protocol`'s stricter criterion. The embedded
//! binding remains unimplemented rather than implied by its configuration vocabulary.
//!
//! **[`host`] is also the definition of the stack's reachable-from-a-call surface** (`X-38`, alpha
//! predicate 1). What this application uses is *Supported*; what no path from it reaches is
//! *Experimental* until a second application disagrees. `scripts/check-app-surface.py` holds the two
//! together, and it reads the application's real dependencies rather than a list — so widening the
//! surface means writing code here that needs it.
//!
//! **Which Cargo features this crate enables is therefore a statement about the stack, not a build
//! detail.** It deliberately enables none of the optional codecs or keying backends. Opus
//! (`sipx-audio/opus`) and the DTLS handshake (`sipx-media/dtls`) each link a C library, and whether
//! to take that on is a deployment decision — a host that made it by default would be answering it
//! for every operator, and would also promote both capabilities onto the supported surface on no
//! evidence beyond a manifest line. They stay experimental and say so on their own pages. Enabling one
//! here is the intended way to change that, and it is a decision with a `CHANGELOG.md` entry rather
//! than a convenience.
//!
//! <!-- BEGIN sipx-api-classification -->
//! **Experimental Rust API roots:**
//!
//! - [`sipx_app::config::vectors`](crate::config::vectors)
//! - [`sipx_app::harness::Binding`](crate::harness::Binding)
//! - [`sipx_app::harness::Conclusion`](crate::harness::Conclusion)
//! - [`sipx_app::harness::DialOutcome`](crate::harness::DialOutcome)
//! - [`sipx_app::harness::Document`](crate::harness::Document)
//! - [`sipx_app::harness::EVENT_QUEUE`](crate::harness::EVENT_QUEUE)
//! - [`sipx_app::harness::Effect`](crate::harness::Effect)
//! - [`sipx_app::harness::EndCause`](crate::harness::EndCause)
//! - [`sipx_app::harness::Event`](crate::harness::Event)
//! - [`sipx_app::harness::EventKind`](crate::harness::EventKind)
//! - [`sipx_app::harness::Gather`](crate::harness::Gather)
//! - [`sipx_app::harness::GatherReason`](crate::harness::GatherReason)
//! - [`sipx_app::harness::Instruction`](crate::harness::Instruction)
//! - [`sipx_app::harness::Outcome`](crate::harness::Outcome)
//! - [`sipx_app::harness::Reply`](crate::harness::Reply)
//! - [`sipx_app::harness::Run`](crate::harness::Run)
//! - [`sipx_app::harness::Scenario`](crate::harness::Scenario)
//! - [`sipx_app::harness::ScriptedApp`](crate::harness::ScriptedApp)
//! - [`sipx_app::harness::Source`](crate::harness::Source)
//! - [`sipx_app::harness::Step`](crate::harness::Step)
//! - [`sipx_app::harness::Verb`](crate::harness::Verb)
//! - [`sipx_app::harness::Virtual`](crate::harness::Virtual)
//! - [`sipx_app::harness::binding`](crate::harness::binding)
//! - [`sipx_app::harness::contract`](crate::harness::contract)
//! - [`sipx_app::harness::scenario`](crate::harness::scenario)
//! - [`sipx_app::harness::time`](crate::harness::time)
//! - [`sipx_app::harness::vectors`](crate::harness::vectors)
//! <!-- END sipx-api-classification -->

// This crate's inline test modules opt out of coverage instrumentation, so the
// published figure measures the code rather than the tests measuring it. Never set outside
// `cargo llvm-cov`, so every other build parses this and discards it. Applied by
// `./scripts/coverage-report.py --annotate`; `docs/coverage.md` states what it costs.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

pub mod config;
/// The host's DSP door (`M-67`): §6.2's `dsp` verbs and §5.3's `call.dsp.*` rows.
///
/// Private because it is the driver's own composition of a contract row and not a surface anybody
/// depends on; [`host`] is the only caller.
mod dsp;
pub mod harness;
pub mod host;
pub mod realtime;
pub mod session;
pub mod webhook;
pub mod wss;
