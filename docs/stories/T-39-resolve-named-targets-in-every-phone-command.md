---
id: T-39
title: "Resolve named targets in every phone command"
pillar: Transport
status: in-progress
epic: endpoint-resolution
areas: [sipx-transport, sipx-cli]
design: docs/designs/endpoint-resolution.md
note: "external review finding 2 · after T-38 · dial, register and scenario accept named targets without manual address injection"
---

# Resolve named targets in every phone command

## Goal

Implement `T-38`'s resolution contract once in the transport layer and make every outbound
diagnostic-phone path accept named SIP targets without external lookup or manual IP injection.

## Acceptance

- [x] `T-38`'s spec is complete. Resolver I/O lives in `sipx-transport`; target ordering remains a
      pure policy tested from injected answer sets and fired timers.
- [x] `dial`, `register`, `load`, registrar-backed `peers` and scenario use the shared resolver.
      A repository check or parser-derived command inventory fails if a later outbound phone path
      bypasses it.
- [x] Named targets with explicit ports resolve A/AAAA records and attempt the bounded ordered
      addresses. SIP server-location records are used exactly where the spec requires them.
- [x] Literal IPv4 and IPv6 targets perform no DNS lookup and preserve their current output,
      transport selection and timing behavior.
- [x] TLS/WSS connects to the selected address while verifying the original hostname. SIPS and
      secure transport never fall back to a cleartext candidate.
- [x] Deterministic tests inject successful, empty, malformed, delayed and mixed-family resolver
      answers; cancellation drops and joins every lookup/attempt task within the spec's bound.
- [ ] A process-level loopback proof calls and registers through a locally controlled hostname,
      while negative proofs distinguish resolution failure, resolution timeout and connection
      failure in text/JSON and exit status.
- [ ] Public CLI/reference examples no longer instruct ordinary named targets to be resolved
      externally. Feature builds, strict Clippy and the complete repository gate are green.

## Review evidence

Finding 2 reproduced the gap through `dial`, derived registration targets and explicit `--target`;
scenario used the same literal-only target derivation after the behavioral result was established.

## Progress

The bounded DNS adapter, one total cross-record LRU cache, shared command resolver, serial
connection-oriented candidate retry and the loopback hostname call/registration proof are in
place. Literal resolution bypasses even system-resolver setup. The complete `sipx-cli` package
suite (108 unit/parser and 57 runnable integration tests), all-feature `sipx-transport` suite (355
tests), no-default transport check, all-feature CLI check, strict package Clippy and provenance
check pass.

The two remaining acceptance rows stay open deliberately. T-37 owns the existing call-layer loss
of an immediate connection cause, which prevents T-39 from claiming the complete process-level
three-way negative proof. Generated docs and the complete repository gate are deferred to the
combined push boundary at the user's request.
