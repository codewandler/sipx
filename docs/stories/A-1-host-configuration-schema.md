---
id: A-1
title: Finish the host configuration and failure-semantics schema
pillar: Application
status: ready
priority: 6
design: docs/designs/app-host.md
epic: app-host
areas: [sipx-app]
note: app-host phase 1 · spec work, no dependency on the app-sdk stories
---

# Finish the host configuration and failure-semantics schema

## Goal
Turn [specs/host-config.md](../specs/host-config.md) from draft to normative: concrete syntax,
listener schema, app/binding/grants/failure tables, reload semantics — with vectors.

## Acceptance
- [ ] The spec's §3 open points are closed and every normative point has at least one vector
      (valid document, rejected document with the reason, reload accepted, reload rejected,
      live-call policy retention across reload).
- [ ] Failure-semantics fields are byte-identical in name and default to
      [`app-contract.md`](../specs/app-contract.md) §9.2.
- [ ] Secrets are by-name references; a vector shows a document with no secret material in it.
- [ ] The multi-app-vs-multi-process stance is recorded as explicitly open with what phase 4
      needs preserved either way.

## Progress
- [specs/host-config.md](../specs/host-config.md) is normative. §2 pins the concrete syntax as a
  named subset of TOML; §3 numbers twelve normative points `N1`…`N12`; §4 gives the listener, app,
  failure and grant tables with types, requirements and defaults; §5 closes the set of refusal
  codes; §6 gives reload; §7 records the open stance; §8 is the vector table `HC-1`…`HC-30`.
- The reader is `crates/sipx-app/src/config/` — `syntax` (the subset), `schema` (§4), `running`
  (§6, admission and reload), `vectors` (§8). It adds no dependency: the subset is hand-read, the
  way the kernel's parsers are.
- Failure semantics are not redefined. `config` reads `harness::policy::FailurePolicy`, and the
  `on_failure` table's key list is derived from `Failure::all()` rather than written out again, so
  a fifth knob in the contract's §9.2 fails a test here instead of becoming an unsettable key.
- Vectors execute: `crates/sipx-app/tests/config_vectors.rs` runs all thirty, and two of them
  (`HC-9`, `HC-28`) run the `A-7` harness — a policy read out of a document has to change what a
  call does, and a live call's captured policy has to survive the reload that redeclared it.
- Two properties are checked over the set rather than by a row: every normative point is named by
  at least one vector, and every refusal code is produced by at least one vector. Adding `N13` or a
  fourteenth code without a vector fails the build.
- Mutation-checked while writing: dropping the topology check, ignoring leftover keys, allowing an
  unrouted listener, allowing `app` and `no_app` together, dropping referential integrity, allowing
  a duplicate key or table, clearing captured policies on reload, and each clause of the secret-name
  grammar — every one fails a vector or a unit test.
- Left for the bindings, and flagged in the spec rather than guessed at: TLS for a `session`
  listener (`A-4`), what an `embedded` handler path resolves against (`A-6`), and what the second
  `signing_secrets` entry does during a rotation window (`A-2`).

## Notes
- No dependency on the `app-sdk` stories — this can run beside them.
