---
id: X-54
title: Count every discard in the signalling path, and let the numbers out beside the capture
pillar: Build
status: in-progress
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
- [x] **The enumeration reaches past one crate.** `no_discard_in_the_signalling_path_is_silent`
      (`crates/sipx-transport/tests/discards.rs`) scans the `src` directory of `sipx-transport` and
      nothing else, so a silent discard added to the dialog layer is invisible to it. The signalling
      path does not stop at the socket: extend the scan — or give `sipx-call` one of the same shape —
      so that the guard covers every crate the clause's words cover. Say in the same change which
      crates those are and why, because "the signalling path" is the term the whole clause turns on.
- [x] **The dialog layer's known losses are counted or reasoned.** `X-51` found these by hand and they
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
- [x] **The numbers are reachable from outside a process, next to the capture.** `Handle::counters`
      and `Calls::counts` are both plain snapshots — that part of `X-18`'s design is right and stays,
      and no metrics library is added. What is missing is that nothing but the crates' own tests ever
      calls either: `--capture <FILE>` puts the traffic where an operator can reach it and there is no
      counterpart for the counts. Give `sipx` one — the shape is the story's choice, recorded with its
      reason — so that "counted and exportable **next to** a capture" is one thing an operator can do
      and not two features that exist separately.
- [x] **One snapshot or a stated reason for two.** `Handle::counters` counts what the transport
      discarded; `Calls::counts` counts what the dispatcher did not deliver. An operator holding a
      capture wants both and should not have to know the crate boundary to ask. Either join them or
      write down why the boundary is load-bearing — the same refusal `X-18` made about media counters
      is a fine answer, and an unstated split is not.
- [x] **Failing-first test**: `a_discard_in_the_dialog_layer_is_counted_next_to_the_capture_of_the_request_that_caused_it`
      — the shape `a_datagram_that_does_not_parse_is_still_captured`
      (`crates/sipx-transport/tests/capture.rs:424`) already has for a parse failure, applied to a loss
      the dialog layer owns. That test is the precedent to follow and the proof the shape works.
- [ ] **`docs/roadmap.md`'s "Where M12 stands" block is updated in the same commit**, and if this
      closes the last gap, M12 moves to Delivered with its four clauses' evidence named. `X-51` wrote
      that block and it becomes wrong the moment this lands.

## Progress
- Filed 2026-07-30 by `X-51`, which checked M12's four **Done when** clauses against the tests and CI
  jobs that are supposed to demonstrate them. Three hold. This is the fourth.
- **Implemented 2026-07-31.** The decision the fourth item asks for is recorded in
  `docs/specs/sip-transport.md` **§12.3**, which is new and is where a resuming agent should start:
  it names the crates the signalling path is, says why the two sets of atomics stay two while the
  reading is one, and states the rule `M-32` extends (a crate joins by being added to `CRATES` and
  by growing a member on `SignallingCounts` — never by adding fields to another crate's struct, and
  never by a second tally of an event already counted).
- **The guard now scans both crates.** `CRATES` in `crates/sipx-transport/tests/discards.rs` lists
  `sipx-transport` and `sipx-call`. Extending it turned up **sixteen** unexplained sites, not the
  seven `X-51` named by hand; all sixteen now carry a counter or a `// discard:` reason.
- **Two new counters, in the crate that owns each fact.** `sipx_transport::UnsentCounts` counts, by
  method, requests handed to `Handle::send`/`send_directly` that never reached the wire — which is
  what covers the six `let _ = …` sends, and every one added later, without a counter having to be
  remembered at each site. `CallEvents::dropped` counts events a consumer was too far behind to be
  given, per call, reported to the only party who can act on it.
- **The export is `sipx --counters <FILE>`, and `--capture <FILE>` implies it** as
  `<capture>.counters.json`. Two rules because two different operators ask: whoever took a capture
  is assembling a bug report and wants the numbers in the same bundle; whoever wants only the
  numbers must not be made to record call content (§13) to get them. The run names the file it
  wrote.
- **Rework round 1 (review findings, both fixed).**
  - *The counter did not count what it claimed.* `UnsentCounts` was incremented in `Handle::send`,
    which returns as soon as the driver has created the transaction — the transmit happens later in
    `perform`. So it could only fire on a closed endpoint or a missing `Via`, and **never** on a
    refused connection, an unreachable peer or an over-MTU datagram, which is the whole of the
    question its own docs said it answered; meanwhile `send_directly` *did* await its transmit, so
    `ack` and `bye` meant different things with nothing saying so. The increment now sits at the two
    places the socket is written. It no longer fires on an ordinary `shutdown` race, and it overlaps
    `DiscardCounts::send_failures` for requests on the transaction path — stated on the type, per
    §12.2. **§12.3 is the rule `M-32` extends, so the corrected rule is "count where the loss
    happens, not where it is reported".**
  - *The export skipped the run that needed it.* `attach` ran only after the call had succeeded, so a
    dial that timed out wrote the capture and no counters. It is now an `Export` guard armed straight
    after `bind`, so every `return fail(…)` takes the file with it.
- **Left for the coordinator:** the last Acceptance item. `docs/roadmap.md` is fenced for this
  worktree, so the "Where M12 stands" replacement prose is in the handoff report under
  `ROADMAP_BLOCK:` rather than committed here. Nothing else in the story is outstanding.

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
