//! The application host: calls terminated on the sipx stack, driven by customer code over the
//! `sipx.app.v1` contract — webhook documents, full-duplex sessions, and an embedded
//! TypeScript runtime as three transports of one vocabulary.
//!
//! The contract is specified in `docs/specs/app-contract.md`, the host's design in
//! `docs/designs/app-host.md`, and the work is tracked by the `A-*` stories on the board.
//!
//! **What exists today is the [`harness`]** (story `A-7`): the deterministic apparatus every later
//! behaviour claim is held to. It runs the contract's own vector set with fake time, a scripted app
//! and scripted call events, which is possible before the call-framework stories land and is the
//! reason it comes first. The bindings (`A-2`, `A-4`) and the host process itself are built against
//! it rather than beside it.

pub mod harness;
