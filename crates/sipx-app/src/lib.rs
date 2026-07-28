//! The application host: calls terminated on the sipx stack, driven by customer code over the
//! `sipx.app.v1` contract — webhook documents, full-duplex sessions, and an embedded
//! TypeScript runtime as three transports of one vocabulary.
//!
//! **Nothing is implemented yet.** The contract is specified in `docs/specs/app-contract.md`,
//! the host's design in `docs/designs/app-host.md`, and the work is tracked by the `A-*`
//! stories on the board. This crate exists so the host has its home in the workspace from day
//! one; the first code arrives with story `A-7` (the deterministic harness) and `A-2` (the
//! document-mode host).
