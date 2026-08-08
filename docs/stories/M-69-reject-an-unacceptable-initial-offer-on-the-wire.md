---
id: M-69
title: "Reject an unacceptable initial offer on the wire"
pillar: Media
status: done
epic: media-interoperability
areas: [sipx-call, sipx-cli]
design: docs/designs/media-interoperability.md
note: "external review finding 3 · no-common-codec failure sends no final SIP response"
---

# Reject an unacceptable initial offer on the wire

## Goal

When an initial INVITE contains a syntactically valid session offer that the selected media policy
cannot accept, send the peer a final 488 response through the existing server transaction before
reporting the local failure.

## Acceptance

- [x] The call/media spec distinguishes malformed SDP, unsupported session policy and internal
      media failure, and maps an unsupported initial offer to RFC 3261 488 with RFC 3264 rationale.
- [x] A failing-first two-process test offers a non-overlapping codec set, captures one INVITE in and
      zero responses out on the current code, and requires a final 488 after the fix.
- [x] The response carries the normal To tag, matching transaction identifiers and Content-Length,
      is retransmitted/absorbed by the server transaction as required, and is sent before local
      command teardown.
- [x] The caller receives the final response and reports `rejected`/exit 3 promptly rather than
      waiting for invitation timeout. The answerer reports the explicit media reason without
      claiming a transport failure.
- [x] A malformed offer follows its separately specified response, and an unacceptable re-INVITE
      retains the existing-dialog 488 behavior without ending working media.
- [x] Failure to transmit the rejection is observable as a send failure and cannot increment a
      successful-response counter.
- [x] Byte-level response vectors, call/CLI process tests, counter/capture assertions and the
      complete repository gate are green.

## Review evidence

Finding 3 observed answer-side `no codec in common`, `messages_in: 1`, `messages_out: 0`, and a
caller that reached timeout because no final response appeared on the wire.

## Progress

- In progress: `docs/specs/call-initial-offer.md` specifies the malformed, unsupported and internal
  failure classes, their initial-INVITE wire mapping and byte-level vectors before implementation.
- Implemented: malformed initial SDP now receives a tagged transaction-owned 400, while a valid
  offer outside the selected codec policy receives a tagged transaction-owned 488 before the
  original typed local error returns. Retransmission, ACK absorption, a late CANCEL and an
  unobservable send failure are pinned at the call boundary; the two-process CLI proof pins both
  exits, elapsed refusal, capture bytes and transport counters.
- Focused validation: the complete `sipx-call` all-feature package suite, the M-69 CLI process test,
  strict all-target/all-feature Clippy for `sipx-call` and `sipx-cli`, formatting, fixed-sleep,
  documentation-link and provenance checks pass. The complete repository gate and derived RFC
  report regeneration remain deliberately deferred to push time.
