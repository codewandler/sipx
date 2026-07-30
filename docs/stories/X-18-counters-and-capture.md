---
id: X-18
title: Count what the stack discards, and capture what it sends
pillar: Build
status: in-progress
priority: 10
design: docs/designs/sip-transport.md
epic: sip-transport
areas: [sipx-transport, sipx-media, sipx-testkit]
note: M12 · nothing leaves the process but tracing; T-19 adds the first counter and has nowhere to put it
---

# Count what the stack discards, and capture what it sends

## Goal
Make a running sipx observable from outside: every discard counted and readable, and the signalling
it exchanged recoverable as a capture that can be attached to a bug report.

## Acceptance

**Counters**
- [x] The endpoint exposes a counter snapshot — a plain type read through the handle, next to
      `Handle::outstanding` — covering at least: requests and responses in and out per transport,
      requests shed (`T-19`), responses that matched no client transaction (`T-18`), parse failures,
      retransmissions sent, and transactions timed out per timer.
- [x] sipx depends on **no** metrics library. Counters are read through a snapshot or a trait, and the
      exporter is the application's choice. A stack that picks an exposition format picks it for every
      user of the library, and it is the one decision here that cannot be undone later.
- [ ] Media counters join them rather than living apart: `M-10` already computes quality statistics,
      and what is missing is that they are per-session and unreachable in aggregate.
      **Not done, and it needs a decision this story cannot make** — see `## Progress`.
- [x] Every existing `let _ = …` discard and every `tracing::debug!("dropped …")` in the signalling
      path has a counter, and a test enumerates them so a new discard added without one is caught. A
      silent drop is the failure this whole story exists to end, and `T-19` fixes only the first one.

**Capture**
- [x] Signalling can be recorded to a file: the messages the endpoint sent and received, with a
      timestamp, the transport, and both addresses. Bodies included — an SDP negotiation that went
      wrong is unreadable without them.
- [x] The format is one standard packet-analysis tooling reads, and *which* format is the story's
      choice, recorded with its reason rather than left implicit.
- [x] Capture is off by default and costs nothing when off. It is opt-in per endpoint, and enabling it
      must not change message ordering or timing — an observation that perturbs a retransmission race
      is worse than no observation.
- [x] TLS, WSS and any future encrypted transport capture the *decrypted* messages, since capturing
      ciphertext from inside the process would be strictly worse than capturing outside it. The story
      says plainly that the resulting file contains credentials and call content.
- [x] `sipx` gains a way to turn it on from the command line, because the
      [vision](../vision.md)'s "testable from a shell" is what makes this usable in an incident rather
      than only in a test. `--capture <FILE>` on `dial`, `answer` and `register`.
- [x] Failing-first test: `a_shed_request_and_an_unmatched_response_both_appear_in_the_counter_snapshot`.

## Progress
- Resumed after the first implementor was stopped mid-flight. What it left was `docs/specs/sip-transport.md`
  §12–§13 and nothing else; no code had been started, which was the right order (`AGENTS.md` §4).
- Spec §12–§13 reviewed rather than inherited. Kept: counters-not-metrics, and atomics shared with the
  handle. Rewritten: §13.2, which specified the write *inline in the driver loop* and argued that this
  was the faithful choice — it is the opposite, since that loop also fires retransmission timers, so an
  inline write puts the filesystem in the retransmission path and perturbs exactly the race the
  Acceptance protects. Ordering is now established at the observation point (sequence + timestamp) and
  the write handed off. Reduced: §13.1 no longer invents per-connection TCP sequence numbers.
  Added: §12.2 (what the counters do not promise) and §13.3 (redaction).
- The starting claim that "no counter leaves the process" was already false when this story was filed:
  `T-19` landed `ShedCounts` and `Handle::shed`.
- **Built (`sipx-transport`).** `crates/sipx-transport/src/counters.rs`: `Handle::counters` returns a
  `Counters` snapshot over shared atomics, synchronous like `Handle::shed` and deliberately unlike the
  `async` `Handle::outstanding` beside it. Per-transport in/out and parse failures, `ShedCounts`
  embedded rather than recounted, unmatched responses, retransmissions counted where the timer fires,
  timeouts split by timer, and six discards that were previously invisible.
  `crates/sipx-transport/src/capture.rs`: pcapng, off by default, sequence and timestamp stamped in the
  loop and the write handed to a thread, redaction on by default.
- **The discard guard is the part worth defending.** `tests/discards.rs` scans the crate's own source
  and fails on a discard with neither a counter nor a `// discard: <reason>` above it. Two of its three
  tests exist to prove the detector can fail, because a guard whose detector is broken looks exactly
  like a clean codebase.
- **The CLI flag landed after `S-30` merged.** `--capture <FILE>` on the three commands that bind an
  endpoint, applied by one `apply_capture` helper in `main.rs` rather than three copies. It is in
  `VALUED_FLAGS`, so it inherits `S-30`'s rule that a valued flag given nothing — or given an empty
  value, which is what an unset shell variable expands to — is a usage error naming the flag rather
  than a run on a default nobody typed. No exception was added for it. `cli.rs`'s
  `a_valued_flag_given_no_value_is_refused_by_every_command` derives its cases from each command's
  help text at run time, so registering the flag and documenting it was enough to cover it.
  **There is deliberately no flag to turn redaction off**, though the library allows it: exposing that
  would put "ship the credentials" one word away from someone debugging an incident at 3am.
- **Media (Acceptance 3) is deliberately not done, and not merely unfinished.** "Join them" hides a
  design decision: `sipx-transport` cannot depend on `sipx-media`, so media counters either need a
  shared counters crate underneath both, or a parallel type of the same shape. The repo has already
  started down the second road — `sipx-call/src/dispatch.rs` carries a "same shape as
  `sipx_transport::ShedCounts`" counter — but that file is in `sipx-call`, which was fenced out of this
  worktree, so the precedent could not be extended from here. Picking the wrong one is expensive to
  undo, which is exactly the kind of choice §12 says to make deliberately.
  The site census is done and is the cheap part of the follow-up: eleven uncounted `tracing` drop
  lines in `sipx-media/src/session.rs` (Opus encode/decode at `:191`/`:235`, SRTP at `:1502`/`:1760`,
  SRTCP at `:2430`/`:2493`, foreign SSRC at `:2157`), a DTMF digit dropped with neither log nor counter
  at `:2578`, an unknown payload type at `:2616`, `Clip::finish` at `:781`, plus five in `ice/driver.rs`
  and two in `ice/gather.rs`. None is reachable from `Counters` today.

## Notes
- `T-19` is the story that makes this urgent rather than nice: its acceptance required a shed request
  to be "counted, and the count […] reachable from outside the endpoint". That is one counter reached
  one way; this is the general case, and doing the general case second is the right order because
  `T-19` was a live fault.
- **Corrected while resuming:** `T-19` has since landed, so the frontmatter note's "adds the first
  counter and has nowhere to put it" describes the situation when this story was filed, not the one it
  is being implemented in. `ShedCounts` exists (`sipx-transport/src/endpoint.rs`) and *is* reachable
  from outside, through `Handle::shed`. That changes nothing about this story's substance — one
  hand-placed counter is not a destination for the next twenty — and it improves it: `Shed` is the
  precedent to follow rather than a thing to invent, and `Handle::shed` versus the `async`
  `Handle::outstanding` beside it is the argument for which shape a snapshot should take.
- The counter list above is not a wish list. Each entry is a place where the current code loses
  information that a support case would need: which transport, how many, and when it started.
- Deliberately not in scope: tracing spans across the whole stack, and any push-based export. Both are
  the application's to build once the numbers exist.
