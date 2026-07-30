---
id: X-54
title: Count every discard in the signalling path, and let the numbers out beside the capture
pillar: Build
status: ready
priority: 3
design: docs/designs/sip-transport.md
epic: sip-transport
areas: [sipx-transport, sipx-call, sipx-cli]
note: found by X-51 — M12's third clause says *every* discard in the signalling path is counted and exportable next to a capture; the enumeration covers one crate, the dialog layer has uncounted losses, and nothing outside the crates' own tests can read either counter snapshot
---

# Count every discard in the signalling path, and let the numbers out beside the capture

## Goal
Close the distance between M12's third **Done when** clause and what `X-18` built, so the milestone
can be recorded on evidence rather than on the clause being nearly true.

## Acceptance
- [ ] **The enumeration reaches past one crate.** `no_discard_in_the_signalling_path_is_silent`
      (`crates/sipx-transport/tests/discards.rs`) scans the `src` directory of `sipx-transport` and
      nothing else, so a silent discard added to the dialog layer is invisible to it. The signalling
      path does not stop at the socket: extend the scan — or give `sipx-call` one of the same shape —
      so that the guard covers every crate the clause's words cover. Say in the same change which
      crates those are and why, because "the signalling path" is the term the whole clause turns on.
- [ ] **The dialog layer's known losses are counted or reasoned.** `X-51` found these by hand and they
      are the census, not a wish list:
      - `crates/sipx-call/src/event.rs:299` — a call event dropped because the consumer is behind,
        reported with `tracing::debug!` and no counter.
      - `crates/sipx-call/src/call.rs:2443` and `:2458` — the result of sending a CANCEL discarded.
      - `crates/sipx-call/src/call.rs:2483`, `:2485`, `:3401` and `:3403` — the result of sending an
        ACK and a BYE discarded.
      The six in `call.rs` are best-effort by design and `ack_then_bye`'s doc comment says so; that is
      a reason and not a count, and §12.1's rule is that a discard is counted. A failed BYE on a
      teardown path is the number an operator asking "why did that call linger" needs. So each site
      gets a counter, or a `// discard: <reason>` in the form the guard reads — which for these means
      arguing that the loss cannot matter, not restating that it was deliberate.
- [ ] **The numbers are reachable from outside a process, next to the capture.** `Handle::counters`
      and `Calls::counts` are both plain snapshots — that part of `X-18`'s design is right and stays,
      and no metrics library is added. What is missing is that nothing but the crates' own tests ever
      calls either: `--capture <FILE>` puts the traffic where an operator can reach it and there is no
      counterpart for the counts. Give `sipx` one — the shape is the story's choice, recorded with its
      reason — so that "counted and exportable **next to** a capture" is one thing an operator can do
      and not two features that exist separately.
- [ ] **One snapshot or a stated reason for two.** `Handle::counters` counts what the transport
      discarded; `Calls::counts` counts what the dispatcher did not deliver. An operator holding a
      capture wants both and should not have to know the crate boundary to ask. Either join them or
      write down why the boundary is load-bearing — the same refusal `X-18` made about media counters
      is a fine answer, and an unstated split is not.
- [ ] **Failing-first test**: `a_discard_in_the_dialog_layer_is_counted_next_to_the_capture_of_the_request_that_caused_it`
      — the shape `a_datagram_that_does_not_parse_is_still_captured`
      (`crates/sipx-transport/tests/capture.rs:424`) already has for a parse failure, applied to a loss
      the dialog layer owns. That test is the precedent to follow and the proof the shape works.
- [ ] **`docs/roadmap.md`'s "Where M12 stands" block is updated in the same commit**, and if this
      closes the last gap, M12 moves to Delivered with its four clauses' evidence named. `X-51` wrote
      that block and it becomes wrong the moment this lands.

## Progress
- Filed 2026-07-30 by `X-51`, which checked M12's four **Done when** clauses against the tests and CI
  jobs that are supposed to demonstrate them. Three hold. This is the fourth.

## Notes
- **This is the whole remaining distance to M12.** The corpus clause is held by `X-16` and `S-31`
  (twelve messages classified, `rfc5118::DEVIATIONS` empty, nineteen green tests), the interop clause
  by `X-17` (two profiles, one CI job each, the same nine-test list), and the fuzzing clause by `X-19`
  and `S-26` (`transaction_sequence` in the `fuzz` job, `KNOWN_DEFECTS` empty).
- **Media is deliberately not in this.** M12's clause says the *signalling* path, so the media
  counters `X-18` split out to `M-32` are outside it, and `M-32` staying open does not hold M12 open.
  That is settled from the clause's own word rather than assumed — see the roadmap block.
- `X-18` is not reopened. It built the counters, the capture and the guard, and refusing the media
  half was the right call. What it could not do from inside its fence was reach `sipx-call`, which was
  fenced out of that worktree — the same reason its third acceptance item became `M-32`.
- The guard's own limit is stated in `docs/specs/sip-transport.md` §12.1 and stays true here: a
  discarded result is found structurally, a logged loss only by the words it uses. Widening the crate
  set does not widen the word list, and both are ratchets rather than proofs.
