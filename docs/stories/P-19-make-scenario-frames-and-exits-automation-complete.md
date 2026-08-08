---
id: P-19
title: "Make scenario frames and exits automation-complete"
pillar: Phone
status: done
epic: diagnostic-automation
areas: [sipx-cli, sipx-app-protocol]
design: docs/designs/diagnostic-automation.md
note: "external review finding 6 · help disagrees with accepted NDJSON and total refusal still exits zero"
---

# Make scenario frames and exits automation-complete

## Goal

Give `sipx scenario` one executable, versioned NDJSON input contract and a process status that tells
a supervisor whether command processing succeeded. No consumer should need source inspection to
discover the frame shape or required wait deadline.

## Acceptance

- [x] A spec under `docs/specs/` defines the canonical flat frame
      `{"id":"…","command":"…",...}`, the accepted `do` compatibility alias if retained, every
      command's required fields, `wait_for.timeout_ms`, response correlation and stream recovery.
- [x] Parser-owned help and the public CLI reference include at least one executable dial/wait/hangup
      transcript and do not describe the rejected one-key-per-command shape.
- [x] Failing-first process tests pin the review cases: the documented nested shape is rejected,
      the flat shape works, malformed JSON and refused commands emit correlated refusal envelopes,
      and a stream containing only failures currently exits 0.
- [x] Clean EOF/shutdown after only successfully accepted operations exits 0. Any malformed frame,
      command refusal or failed operation makes the final process exit nonzero after all correlated
      envelopes have been flushed; a deliberate successful `reject` operation is not itself an
      infrastructure failure.
- [x] Mixed streams continue after a bad frame, preserve input/event order, and still exit nonzero
      so one later success cannot hide an earlier unhandled refusal.
- [x] An empty stream, duplicate IDs, missing finite wait deadline and unknown command have explicit
      typed outcomes and cannot panic, desynchronize later frames or create unbounded waits.
- [x] The scenario command remains bounded and cancellation-safe; focused protocol/process tests,
      docs synchronization and the complete repository gate are green.

## Review evidence

Finding 6 showed help naturally implied `{"id":"1","dial":{...}}` while only a flat string
`command`/`do` field worked. Invalid JSON, every command refused and failed dial streams all exited
zero; `wait_for`'s required `timeout_ms` was absent from help.

## Progress

- The canonical frame, compatibility selector, per-command fields, correlation, recovery, terminal
  stream result and exit mapping were specified in `docs/specs/scenario-automation.md` before
  implementation.
- Failing-first: a stream containing four correlated refusals emitted every expected envelope but
  exited 0 and had no terminal stream outcome before the fix.
- The actor now remembers any frame or operation failure, continues at line boundaries, rejects
  duplicate correlations, joins calls/media/the endpoint, emits one terminal stream event and
  derives exit 0/1 from the complete stream. A deliberately successful SIP `reject` remains a
  completed operation.
- Parser-owned help and the public reference carry the same copyable flat dial/wait/hangup/shutdown
  transcript and field vocabulary.
- Focused validation is green: six scenario process tests, strict all-target/all-feature clippy,
  CLI-reference, docs-link and fixed-sleep checks, and `cargo test -p sipx-cli --all-features`.
- Per the working-session instruction, derived regeneration and the complete gate are deferred to
  push time; the final acceptance item therefore remains open.
