---
id: S-8
title: Implement the transaction layer and message matching
pillar: Signalling
status: backlog
priority:
design: docs/designs/sip-core.md
epic: sip-core
areas: [sipx-sip]
note:
---

# Implement the transaction layer and message matching

## Goal
Route incoming messages to the right transaction — or to the application when there is none —
including the pre-RFC-3261 senders still on the internet.

## Acceptance
- [ ] Server transaction matching per RFC 3261 §17.2.3: `branch` with the magic cookie when
      present, and the legacy §17.2.3 fallback (request URI, `To` tag, `From` tag, `Call-ID`,
      `CSeq`, top `Via`) when it is not.
- [ ] Client transaction matching per §17.1.3 on `branch` plus `CSeq` method.
- [ ] CANCEL matches the transaction of the request it cancels, not a transaction of its own.
- [ ] Transactions are removed on termination with no leak: a test creates and terminates
      10 000 transactions and asserts the stores are empty.
- [ ] Messages matching no transaction are surfaced to the application rather than dropped.
- [ ] Failing-first test: `legacy_branch_matching_rfc2543_fallback`, using a request with no
      magic cookie.

## Progress
- Not started.

## Notes
- The RFC 4475 corpus contains 2543-style messages; wire them into this story's tests.
