---
id: P-11
title: Drive interactive call actions through a correlated NDJSON protocol
pillar: Phone
status: done
priority: 9
design: docs/designs/phone.md
epic: phone
areas: [sipx-cli, sipx-call]
announcement: 2
note: after P-8/P-9; includes validated custom headers and no sleep command
---

# Drive interactive call actions through a correlated NDJSON protocol

## Goal

Make in-call behavior scriptable without baking one fixed sequence into each subcommand.

## Acceptance

- [x] `sipx scenario` implements the command, correlation, EOF and error rules in
      `diagnostic-phone.md` §4 using the existing versioned event envelope.
- [x] `--header` implements §3's validation and stack-owned-field refusal for ordinary commands and
      scenario-originated requests.
- [x] Waits are event predicates with finite deadlines; no fixed sleep stands in for readiness.
- [x] Invalid input cannot corrupt stdout or abandon an active call.
- [x] `DPH-8` and `DPH-9` fail first and pass from a shell pipeline.

## Progress

- `dial`, `answer`, `register` and scenario-originated INVITEs accept repeatable validated headers;
  stack-owned names and compact forms are refused before the applicable bind/dial boundary, and
  retries/refreshes preserve application-owned fields.
- The one-call scenario actor implements all fourteen §4 commands over correlated `sipx.app.v1`
  NDJSON. Event waits require `timeout_ms`; partial malformed frames retain an unambiguous id; EOF
  joins bounded recording/playback and terminates calls or pending invitations.
- DPH-8 observes a custom Supported value on the wire and proves an injected Via loses before local
  address parsing. DPH-9 pipes dial → wait-for-answer → DTMF → hangup → shutdown through the real
  binary and asserts monotonically sequenced causal events. Both pass without fixed readiness sleeps.
