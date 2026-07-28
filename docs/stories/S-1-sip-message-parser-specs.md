---
id: S-1
title: Specify the SIP message model and parser
pillar: Signalling
status: done
priority:
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
- [x] `docs/specs/sip-message.md` defines: the request/response types, the zero-copy
      representation over `Bytes` with a header index, lazy typed header access, the header
      name model (compact forms per RFC 3261 §7.3.3), and the error enum.
- [x] `docs/specs/sip-parser.md` defines: the ABNF subset in scope (RFC 3261 §25), the
      start-line and header grammar, line folding, `Content-Length` handling, the streaming
      framing rules for reliable transports, and the exact conditions under which a message
      is rejected.
- [x] Every normative statement cites an RFC section or is explicitly marked as a project
      decision with its rationale.
- [x] Both specs end with byte-level test vectors — at least 10 accept and 10 reject cases —
      that `S-2`…`S-4` implement against verbatim.
- [x] Ambiguities the RFC leaves open (unknown headers, duplicate `Content-Length`,
      whitespace tolerance) are each decided, in writing, with a stated reason.

## Progress
- Done. Both specs written, every decision tagged [RFC] or [sipx] with rationale.
- Key decisions settled: CRLF-only line endings (bare LF is a smuggling vector); percent
  escapes are never decoded during parsing; repeated `Content-Length` is rejected even when
  the values agree; structural parsing does not validate application semantics, so a message
  missing required headers still parses and can be answered with a 400.
- The `ParseErr` vs `HeaderErr` split fell out of the corpus: `ncl` is a framing failure while
  `scalar02` parses fine and has a bad `CSeq`. Conflating them would either drop forwardable
  messages or accept unframeable ones.

## Notes
- Reject decisions must be reachable from the RFC 4475 corpus imported in `X-2`; cross-check
  the two once both exist.
