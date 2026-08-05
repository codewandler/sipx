---
id: T-39
title: "Resolve named targets in every phone command"
pillar: "Transport"
status: ready
priority: 1
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

- [ ] `T-38`'s spec is complete. Resolver I/O lives in `sipx-transport`; target ordering remains a
      pure policy tested from injected answer sets and fired timers.
- [ ] `dial`, `register`, `load`, registrar-backed `peers` and scenario use the shared resolver.
      A repository check or parser-derived command inventory fails if a later outbound phone path
      bypasses it.
- [ ] Named targets with explicit ports resolve A/AAAA records and attempt the bounded ordered
      addresses. SIP server-location records are used exactly where the spec requires them.
- [ ] Literal IPv4 and IPv6 targets perform no DNS lookup and preserve their current output,
      transport selection and timing behavior.
- [ ] TLS/WSS connects to the selected address while verifying the original hostname. SIPS and
      secure transport never fall back to a cleartext candidate.
- [ ] Deterministic tests inject successful, empty, malformed, delayed and mixed-family resolver
      answers; cancellation drops and joins every lookup/attempt task within the spec's bound.
- [ ] A process-level loopback proof calls and registers through a locally controlled hostname,
      while negative proofs distinguish resolution failure, resolution timeout and connection
      failure in text/JSON and exit status.
- [ ] Public CLI/reference examples no longer instruct ordinary named targets to be resolved
      externally. Feature builds, strict Clippy and the complete repository gate are green.

## Review evidence

Finding 2 reproduced the gap through `dial`, derived registration targets and explicit `--target`;
scenario used the same literal-only target derivation after the behavioral result was established.
