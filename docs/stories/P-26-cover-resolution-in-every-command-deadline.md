---
id: P-26
title: Cover resolution in every command deadline
pillar: Phone
status: ready
priority: 15
design:
epic: diagnostic-automation
areas: [sipx-cli, sipx-transport]
predicate:
announcement:
note: dial --timeout starts its clock after resolution · a slow name can spend the resolver's eight seconds before the invitation is even sent
---

# Cover resolution in every command deadline

## Goal

Make every outbound command's stated deadline cover target resolution, the way `register`'s already
does. A caller that asks for five seconds should get an answer in five seconds, not five seconds
after an unbounded name lookup finished.

## Acceptance

- [ ] `dial`, `load`, `peers` and `scenario` derive their resolver from the command's own deadline
      rather than `Resolver::system()`, so the resolution budget is inside the stated bound and not
      additional to it.
- [ ] A failing-first test proves each command returns near its configured budget against a
      black-holing nameserver, not budget plus the resolver's overall deadline.
- [ ] The published reference states, for every command carrying a deadline, that the deadline
      covers resolution — and the statement is checked rather than prose.
- [ ] A resolution timeout stays distinguishable from a resolution failure and from a connection
      failure in text, JSON and exit status, exactly as `T-39` established.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `T-39`'s implementation. `crates/sipx-cli/src/dial.rs:59` calls
  `Resolver::system()` rather than `Resolver::within()`, so `dial --timeout 5` against a slow name
  can spend the resolver's eight-second overall deadline *before* the invitation clock starts.
  `P-25` fixed exactly this shape for `register`; `T-39` documented the bound honestly rather than
  claiming it for `dial`, and left the rest to this story.

## Notes

- `P-25` is the reference implementation: `Resolver::within` at `crates/sipx-cli/src/destination.rs`
  clamps `T-38`'s per-question and whole-resolution deadlines under the caller's budget.
- Do not add a second clock. The resolver already owns bounded resolution; this story only makes the
  command's budget the ceiling over it.
