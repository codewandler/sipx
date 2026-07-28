---
id: S-8
title: Implement the transaction layer and message matching
pillar: Signalling
status: done
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
- [x] Server transaction matching per RFC 3261 §17.2.3: `branch` with the magic cookie when
      present, and the legacy §17.2.3 fallback (request URI, `To` tag, `From` tag, `Call-ID`,
      `CSeq`, top `Via`) when it is not.
- [x] Client transaction matching per §17.1.3 on `branch` plus `CSeq` method.
- [x] CANCEL matches the transaction of the request it cancels, not a transaction of its own.
- [x] Transactions are removed on termination with no leak: a test creates and terminates
      10 000 transactions and asserts the stores are empty.
- [x] Messages matching no transaction are surfaced to the application rather than dropped.
- [x] Failing-first test: `legacy_branch_matching_rfc2543_fallback`, using a request with no
      magic cookie.

## Progress
- Done. `crates/sipx-sip/src/transaction/{key,layer}.rs`.
- ACK and CANCEL both fold to `INVITE` when deriving a key, because each matches the
  transaction of the request it refers to rather than starting one of its own.
- An unmatched response is passed up rather than dropped: it may be a stray fork answer, and
  an unmatched ACK for a 2xx is entirely normal under RFC 6026.
- The leak test creates and retires 10 000 transactions and asserts both stores are empty. A
  transaction store that leaks is a slow, quiet outage.

## Notes
- The RFC 4475 corpus contains 2543-style messages; wire them into this story's tests.
