---
id: S-1
title: Specify the SIP message model and parser
pillar: Signalling
status: ready
priority: 1
design: docs/designs/sip-core.md
epic: sip-core
areas: [sipx-sip]
note: gates every other sip-core story
---

# Specify the SIP message model and parser

## Goal
Write the implementable contract for how sipx represents and parses SIP messages, so the
rest of the core is derived from a reviewed document rather than discovered in code.

## Acceptance
- [ ] `docs/specs/sip-message.md` defines: the request/response types, the zero-copy
      representation over `Bytes` with a header index, lazy typed header access, the header
      name model (compact forms per RFC 3261 §7.3.3), and the error enum.
- [ ] `docs/specs/sip-parser.md` defines: the ABNF subset in scope (RFC 3261 §25), the
      start-line and header grammar, line folding, `Content-Length` handling, the streaming
      framing rules for reliable transports, and the exact conditions under which a message
      is rejected.
- [ ] Every normative statement cites an RFC section or is explicitly marked as a project
      decision with its rationale.
- [ ] Both specs end with byte-level test vectors — at least 10 accept and 10 reject cases —
      that `S-2`…`S-4` implement against verbatim.
- [ ] Ambiguities the RFC leaves open (unknown headers, duplicate `Content-Length`,
      whitespace tolerance) are each decided, in writing, with a stated reason.

## Progress
- Not started.

## Notes
- Reject decisions must be reachable from the RFC 4475 corpus imported in `X-2`; cross-check
  the two once both exist.
