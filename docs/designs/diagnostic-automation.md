---
id: diagnostic-automation
---

# Reliable diagnostic automation

**Status:** proposed · **Pillar:** Phone/Build · **Epic:** `diagnostic-automation` ·
**Review:** [external functionality and usability review](../reviews/extern-2026-08-06T01-18-47+02-00-full-sweep.md)
findings 4–7 and 12–14 · **Stories:** `X-110`, `P-18`, `P-19`, `P-20`, `P-21`

## Problem

The diagnostic phone promises a shell-testable product boundary, so a plausible command that exits
zero after doing no useful work is worse than an explicit refusal. The review found five versions
of that problem: unknown options were ignored; examples placed global-looking options where the old
parser rejected them; the two load tools did not interoperate with their defaults; scenario help
did not describe its accepted frames and total refusal still exited zero; recording destinations
failed only after the call; and one JSON result emitted a duplicate key.

## Invariants

- One typed parser owns command syntax, global options, environment fallbacks and help. Unknown or
  malformed syntax exits usage before I/O. Review finding 4 used an unsupported `register
  --timeout`; the resolution is to reject it, not silently promise a new registration timer.
- Every documented example is executable against parser-generated help. Global `--json` and `-v`
  have one placement rule rather than a troubleshooting-page exception.
- Paired diagnostic commands have compatible defaults. A successful load summary means the
  configured admission work actually ran; an internal media or policy mismatch cannot masquerade
  as an operator interruption.
- Scenario has a versioned NDJSON command contract with one canonical frame shape, explicit
  required fields and a terminal exit derived from processing outcomes.
- Inputs whose failure is knowable before network I/O, including an unusable recording
  destination, fail before the call. Work that remains subject to a filesystem race reports the
  later failure without discarding already captured data.
- Structured results contain unique keys while retaining deterministic field order and the
  stdout/result, stderr/diagnostic separation.

## Story ownership

`X-110` owns findings 4, 7 and 12 because a declarative global parser rejects the unsupported
registration option and misspellings, accepts global options consistently, and drives generated
help. `P-18` owns the default load pairing, `P-19` the scenario protocol and exit, `P-20` recording
preflight, and `P-21` unique result fields. The review findings are deliberately not duplicated
into one-story-per-symptom parser defects.

## Exit

Every public example parses, every refusal is local and actionable, successful exits correspond to
completed requested work, scenario supervisors can trust the process status, recording setup is
validated before signalling, and strict duplicate-key checks accept every JSON result.
