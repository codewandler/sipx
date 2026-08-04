//! The application host — **a host process that answers a call, plus the design and test harness it
//! was built against.**
//!
//! Calls terminated on the sipx stack are driven by customer code over the `sipx.app.v1` contract.
//! [`host`] and [`webhook`] implement document mode end to end; full-duplex sessions and an embedded
//! TypeScript runtime remain later transports of the same vocabulary. Configuration and the
//! deterministic harness live beside the host rather than inside the contract interpreter.
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
//! sipx is pre-1.0, so **neither word below means frozen**. `1.0.0` is what freezes an API, and its
//! predicates are in `docs/roadmap.md`. Until then:
//!
//! - **Supported** — meant to be depended on. Breaking changes get a `CHANGELOG.md` entry saying what
//!   to do instead. New enum variants and new struct fields may still appear in a minor release, so a
//!   downstream `match` should carry a `_` arm.
//! - **Experimental** — may change shape or be removed without a migration note. Depend on it only if
//!   you are prepared to follow it.
//!
//! An experimental item **graduates** when something outside this repository depends on it: a second
//! caller is what constrains a shape, so the answer to "somebody is using it" is a `CHANGELOG.md`
//! entry moving it to *Supported*, not a note asking them to stop. Without that clause this line
//! would be a freeze rather than a measurement (`X-38`).
//!
//! **Supported:** the document-mode path through [`host`], [`webhook`], and the configuration it
//! consumes. A webhook in another process now drives a real call through `sipx.app.v1`; changes to
//! that host-facing API receive a changelog entry and migration guidance. Session and embedded
//! bindings remain unimplemented rather than implied by their configuration vocabulary.
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

pub mod config;
pub mod harness;
pub mod host;
pub mod webhook;
