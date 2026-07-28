---
id: S-12
title: Implement reliable provisional responses
pillar: Signalling
status: ready
priority: 7
design:
epic: conformance
areas: [sipx-sip, sipx-call]
note: track: signalling · RFC 3262 · same files as S-11, take it second
---

# Implement reliable provisional responses

## Goal
100rel and PRACK, which some carriers require before they will accept a call at all.

## Acceptance
- [ ] `100rel` is offered in `Supported` and honoured when the far end puts it in `Require`.
- [ ] A reliable provisional carries `RSeq` and is retransmitted until PRACK acknowledges it.
- [ ] PRACK carries the matching `RAck` and is answered.
- [ ] A far end that requires `100rel` when sipx has it disabled is refused with 420 naming the
      option tag, rather than left to time out.
- [ ] An offer in a reliable provisional is answered in the PRACK (RFC 3262 §5).
- [ ] Failing-first test: `a_reliable_provisional_is_retransmitted_until_pracked`.

## Progress
- Not started. `RAck` and `RSeq` parse already.
