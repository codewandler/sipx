---
id: P-19
title: "Make scenario frames and exits automation-complete"
pillar: "Phone"
status: ready
priority: 1
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

- [ ] A spec under `docs/specs/` defines the canonical flat frame
      `{"id":"…","command":"…",...}`, the accepted `do` compatibility alias if retained, every
      command's required fields, `wait_for.timeout_ms`, response correlation and stream recovery.
- [ ] Parser-owned help and the public CLI reference include at least one executable dial/wait/hangup
      transcript and do not describe the rejected one-key-per-command shape.
- [ ] Failing-first process tests pin the review cases: the documented nested shape is rejected,
      the flat shape works, malformed JSON and refused commands emit correlated refusal envelopes,
      and a stream containing only failures currently exits 0.
- [ ] Clean EOF/shutdown after only successfully accepted operations exits 0. Any malformed frame,
      command refusal or failed operation makes the final process exit nonzero after all correlated
      envelopes have been flushed; a deliberate successful `reject` operation is not itself an
      infrastructure failure.
- [ ] Mixed streams continue after a bad frame, preserve input/event order, and still exit nonzero
      so one later success cannot hide an earlier unhandled refusal.
- [ ] An empty stream, duplicate IDs, missing finite wait deadline and unknown command have explicit
      typed outcomes and cannot panic, desynchronize later frames or create unbounded waits.
- [ ] The scenario command remains bounded and cancellation-safe; focused protocol/process tests,
      docs synchronization and the complete repository gate are green.

## Review evidence

Finding 6 showed help naturally implied `{"id":"1","dial":{...}}` while only a flat string
`command`/`do` field worked. Invalid JSON, every command refused and failed dial streams all exited
zero; `wait_for`'s required `timeout_ms` was absent from help.
