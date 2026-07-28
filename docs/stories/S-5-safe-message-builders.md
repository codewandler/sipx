---
id: S-5
title: Build requests and responses so injection is unrepresentable
pillar: Signalling
status: backlog
priority:
design: docs/designs/sip-core.md
epic: sip-core
areas: [sipx-sip]
note:
---

# Build requests and responses so injection is unrepresentable

## Goal
Make it impossible to construct a message that smuggles CRLF — or any other structural
character — out of user-supplied data, by construction rather than by a validation call the
caller might forget.

## Acceptance
- [ ] `Request` and `Response` builders accept only typed components; there is no public API
      that appends a raw header line from an unvalidated string.
- [ ] Header values containing CR, LF, NUL or an unescaped structural character are rejected
      at build time with a typed error, not silently escaped or truncated.
- [ ] The same guarantee holds for URI components, display names and reason phrases.
- [ ] Failing-first test: `crlf_injection_rejected_in_every_user_supplied_field`, driven by a
      table of every field a caller can populate — so a newly added field with no guard fails
      the test.
- [ ] Response construction from a request copies `Via`, `From`, `To`, `Call-ID` and `CSeq`
      per RFC 3261 §8.2.6.2, preserving `Via` order.

## Progress
- Not started.

## Notes
- The table-driven test is the point: guarding today's fields is easy, guarding tomorrow's is
  what fails in practice.
