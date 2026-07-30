//! The application host — **as a design and a test harness; there is no host process yet.**
//!
//! What it will be: calls terminated on the sipx stack, driven by customer code over the
//! `sipx.app.v1` contract, with webhook documents, full-duplex sessions and an embedded
//! TypeScript runtime as three transports of one vocabulary. What is here today is the
//! configuration types and the harness, and nothing that answers a call.
//!
//! The contract is specified in `docs/specs/app-contract.md`, the host's design in
//! `docs/designs/app-host.md`, and the work is tracked by the `A-*` stories on the board.
//!
//! **What exists today is the [`harness`]** (story `A-7`): the deterministic apparatus every later
//! behaviour claim is held to. It runs the contract's own vector set with fake time, a scripted app
//! and scripted call events, which is possible before the call-framework stories land and is the
//! reason it comes first. The bindings (`A-2`, `A-4`) and the host process itself are built against
//! it rather than beside it.
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
//!
//! **Experimental**, and mostly absent: there is no host process. What settles it is the host existing
//! and terminating a call.

pub mod config;
pub mod harness;
