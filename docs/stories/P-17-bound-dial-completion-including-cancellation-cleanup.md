---
id: P-17
title: "Bound dial completion including cancellation cleanup"
pillar: "Phone"
status: in-progress
epic: phone-lifecycle
areas: [sipx-cli, sipx-call]
design: docs/designs/phone-lifecycle.md
note: "external review finding 8 · every configured timeout carries an unexplained two-second tail"
---

# Bound dial completion including cancellation cleanup

## Goal

Make the dial command's advertised timeout an honest process-level bound. The INVITE wait and the
cleanup triggered when that wait expires must have explicit budgets and measurements; a fixed
post-timeout tail may not be hidden behind an error that names only the requested timeout.

## Acceptance

- [x] `docs/specs/diagnostic-phone.md` defines whether `--timeout` is the total deadline or names a
      separate cancellation-cleanup allowance, including zero semantics, precedence with final
      responses, and the terminal report fields for both phases.
- [x] Failing-first paused-time tests reproduce the constant cleanup tail for 1, 2, 3, 5 and 8
      second invitation budgets without wall-clock sleeps, then hold every terminal path to the new
      state table.
- [x] A non-answering UDP target cannot keep the process beyond the documented total bound. Expiry
      sends RFC 3261 CANCEL when a client transaction exists and reaches a finite join barrier even
      when neither CANCEL nor INVITE receives a response.
- [x] A final response that happens before the deadline wins; one that happens after timeout cannot
      turn the result back into success. Exact-boundary ordering is deterministic under paused time.
- [x] Result text and JSON report the actual elapsed wait and cleanup facts and never claim that the
      configured invitation threshold alone was the process's elapsed bound when it was not.
- [x] No fixed wall-clock wait substitutes for transaction completion or an owned-task join; any
      failure cap is classified with the repository's accepted inline reason.
- [ ] Focused default/all-feature tests, CLI documentation and the complete repository gate are
      green.

## Review evidence

Finding 8 measured 1, 2, 3, 5 and 8 second requests returning at approximately 3, 4, 5, 7 and 10
seconds respectively, consistent with one unreported two-second cancellation tail.

## Progress

- The diagnostic-phone contract now preserves `--timeout` as the invitation-answer phase and names
  a separate cancellation allowance, including zero and exact-boundary semantics, terminal fields
  and the endpoint join barrier. Board regeneration and the complete gate remain deferred to the
  requested push boundary.
- `DialOptions` carries the distinct cancellation allowance and returns measured invitation and
  cleanup state. The cleanup state records whether CANCEL was admitted, a crossed final response
  was observed, transaction completion occurred, or the explicit allowance was exhausted.
- Paused-time tests cover 1, 2, 3, 5 and 8 second answer limits, zero cleanup, and final responses
  before, exactly at and after the deadline. The shipped-process test observes CANCEL at a ringing
  peer, holds the command to its two-phase bound and checks every JSON timing/cleanup field.
- Focused default/all-feature tests, strict all-target/all-feature Clippy, executable CLI-reference
  agreement, fixed-wait policy, docs links and provenance pass. The complete gate and final status
  remain deferred to the requested push boundary.
