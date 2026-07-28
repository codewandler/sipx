---
id: S-5
title: Build requests and responses so injection is unrepresentable
pillar: Signalling
status: done
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
- [x] `Request` and `Response` builders accept only typed components; there is no public API
      that appends a raw header line from an unvalidated string.
- [x] Header values containing CR, LF, NUL or an unescaped structural character are rejected
      at build time with a typed error, not silently escaped or truncated.
- [x] The same guarantee holds for URI components, display names and reason phrases.
- [x] Failing-first test: `crlf_injection_rejected_in_every_user_supplied_field`, driven by a
      table of every field a caller can populate — so a newly added field with no guard fails
      the test.
- [x] Response construction from a request copies `Via`, `From`, `To`, `Call-ID` and `CSeq`
      per RFC 3261 §8.2.6.2, preserving `Via` order.

## Progress
- Done. `crates/sipx-sip/src/build.rs`. `Header::new` is now `new_unchecked` and crate-private;
  the only public constructor is `Header::build`, which is fallible. The parser keeps the
  unchecked path because it works on bytes that were already framed.
- The table-driven test covers six payloads across six fields. It is the acceptance criterion
  that actually matters: guarding today's fields is easy, and the real failure mode is a field
  added next year with no guard.
- `body()` sets `Content-Length` with the body and replaces any existing one, so a builder
  cannot produce an unframeable message. `build()` adds `Content-Length: 0` when there is no
  body, since a stream transport cannot frame without it.
- `ResponseBuilder::to_request` copies `Via` in order and copies header *bytes* rather than
  re-deriving them, which means a request with an unparseable `To` still gets a well-formed
  400. There is a test for exactly that.

## Notes
- The table-driven test is the point: guarding today's fields is easy, guarding tomorrow's is
  what fails in practice.
