---
id: P-11
title: Drive interactive call actions through a correlated NDJSON protocol
pillar: Phone
status: backlog
priority: 9
design: docs/designs/phone.md
epic: phone
areas: [sipx-cli, sipx-call]
note: after P-8/P-9; includes validated custom headers and no sleep command
---

# Drive interactive call actions through a correlated NDJSON protocol

## Goal

Make in-call behavior scriptable without baking one fixed sequence into each subcommand.

## Acceptance

- [ ] `sipx scenario` implements the command, correlation, EOF and error rules in
      `diagnostic-phone.md` §4 using the existing versioned event envelope.
- [ ] `--header` implements §3's validation and stack-owned-field refusal for ordinary commands and
      scenario-originated requests.
- [ ] Waits are event predicates with finite deadlines; no fixed sleep stands in for readiness.
- [ ] Invalid input cannot corrupt stdout or abandon an active call.
- [ ] `DPH-8` and `DPH-9` fail first and pass from a shell pipeline.

## Progress

- Not started.
