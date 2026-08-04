---
id: X-95
title: Order the symmetric-RTP proof by receipt, not kernel enqueue
pillar: Build
status: done
priority: 1
design: docs/specs/deployment-addresses.md
epic: conformance
areas: [sipx-media, ci]
predicate: 3
announcement:
note: exact-main beta.4 CI exposed a send-to-versus-receive race in the non-ICE proof
---

# Order the symmetric-RTP proof by receipt, not kernel enqueue

## Goal

Make the non-ICE symmetric-RTP proof express the product's causal contract: returned media moves
to an observed source only after sipx has accepted a valid packet from it.

## Acceptance

- [x] The hosted failure is preserved as failing-first evidence and identified as a test-ordering
      defect rather than a production symmetric-RTP regression.
- [x] The proof waits for sipx's public receive event before sending returned media; no fixed sleep
      or widened race window stands in for the event.
- [x] The isolated regression remains green under a bounded repetition, and the complete gate and
      exact-main CI pass before the beta.4 tag is created.

## Progress

- Filed from exact-main workflow run `30953042323`, job `92139530954`. The test's RTP primer reached
  the kernel through `send_to`, but the test queued 100 reply frames before sipx's receive loop had
  necessarily parsed the primer and updated the shared destination. One legal pre-latch frame then
  reached the advertised sink and failed the assertion.
- The unchanged test reproduced twice in 200 bounded runs. After it waited for `MediaSession::recv`
  before sending one reply, the exact regression passed 200 of 200 repetitions and the complete
  local gate passed all 36 steps.

## Notes

- `docs/specs/deployment-addresses.md` §5 and `docs/specs/ice.md` §13.3 require the source switch
  after a valid RTP packet is accepted, not after the peer's socket reports a successful send.
