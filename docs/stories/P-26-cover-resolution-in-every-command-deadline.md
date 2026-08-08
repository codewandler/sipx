---
id: P-26
title: Cover resolution in every command deadline
pillar: Phone
status: done
priority:
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

- [x] `dial`, `load`, `peers` and `scenario` derive their resolver from the command's own deadline
      rather than `Resolver::system()`, so the resolution budget is inside the stated bound and not
      additional to it.
- [x] A failing-first test proves each command returns near its configured budget against a
      black-holing nameserver, not budget plus the resolver's overall deadline.
- [x] The published reference states, for every command carrying a deadline, that the deadline
      covers resolution — and the statement is checked rather than prose.
- [x] A resolution timeout stays distinguishable from a resolution failure and from a connection
      failure in text, JSON and exit status, exactly as `T-39` established.
- [x] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `T-39`'s implementation. `crates/sipx-cli/src/dial.rs:59` calls
  `Resolver::system()` rather than `Resolver::within()`, so `dial --timeout 5` against a slow name
  can spend the resolver's eight-second overall deadline *before* the invitation clock starts.
  `P-25` fixed exactly this shape for `register`; `T-39` documented the bound honestly rather than
  claiming it for `dial`, and left the rest to this story.
- 2026-08-08: implemented. `Resolver::within` now takes `Option<Duration>` — `None` being a command
  that states no deadline — and `Resolver::system` is private, so a command cannot resolve without
  saying what it resolves under; `destination.rs`'s inventory test asserts exactly that for all five
  outbound commands. `Resolver::narrowed` derives a tighter policy from an existing resolver so
  `scenario` can bound each `dial` frame by its own `timeout_ms` without discarding the actor's DNS
  cache between calls. Budgets: `dial --timeout`, `load --timeout` (the setup bound its calls
  share), `scenario`'s per-frame `timeout_ms` or `--timeout`, and — since `peers` states no attempt
  deadline at all — `peers --expires`, the only duration its caller gives it. `peers` also validates
  `--expires` before the lookup rather than after it, because that value is now the lookup's bound.
- 2026-08-08: **the invitation phase is still funded from a fresh copy of the stated budget**, not
  from what resolution left, for `dial`, `load` and `scenario`. Resolution is bounded *by* the
  deadline rather than subtracted *from* it, so the worst case is two phases of the stated value
  rather than one — `register` alone funds each candidate from the remainder (`P-25`). Closing that
  gap changes what `invitation_limit_ms`/`invitation_elapsed_ms` mean and rewrites
  `diagnostic-phone.md` §3.2's "starts when the initial INVITE is handed to the endpoint", which is
  published surface this story did not sanction touching. §3.2 now states the split explicitly; a
  follow-up story should decide whether `dial` adopts `register`'s remainder accounting.

- 2026-08-08: closed in the `1.0.0-rc.5` boundary.

## Notes

- `P-25` is the reference implementation: `Resolver::within` at `crates/sipx-cli/src/destination.rs`
  clamps `T-38`'s per-question and whole-resolution deadlines under the caller's budget.
- Do not add a second clock. The resolver already owns bounded resolution; this story only makes the
  command's budget the ceiling over it.
