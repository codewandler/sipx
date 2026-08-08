---
id: S-54
title: One framing rule for both WebSocket message parsers
pillar: Session
status: backlog
priority: 45
design: docs/specs/sip-tls.md
epic: browser-sdk
areas: [wasm, websocket, browser, parsing]
predicate:
announcement:
note: S-53 left the RFC 7118 §5 rule implemented twice · ws.rs already warns about exactly this
---

# One framing rule for both WebSocket message parsers

## Goal

Give `docs/specs/sip-tls.md` §4's "one WebSocket message is one SIP message" rule a single
implementation, instead of the two that `S-53` left behind, so the browser kernel and the native
WebSocket transport cannot drift apart about what a well-framed frame is.

## Acceptance

- [ ] `crates/sipx-transport/src/ws.rs`'s `parse_one` and `crates/sipx-wasm/src/kernel/mod.rs`'s
      `parse_frame` are one function, in `sipx-sip`, reachable with default features off so the
      `wasm32-unknown-unknown` build still resolves no entropy source, clock or socket.
- [ ] It is public API on a published crate, so it carries rustdoc saying what it promises — one
      whole message and nothing else — and what it does not: it is not the datagram tolerance of
      RFC 3261 §18.3, and it does not require `Content-Length`.
- [ ] A differential test drives both callers over the same frames, including the two shapes that
      separate this rule from `parse_datagram` (two complete messages; one message plus trailing
      octets) and the two that separate it from the stream framer (no `Content-Length`; a leading
      CRLF keep-alive).
- [ ] Neither caller's observable behaviour changes: `sipx-wasm`'s corpus replay digest and
      `sipx-transport`'s WS framing cases are unchanged, and the change is visible only as a diff
      that deletes one of the two copies.
- [ ] `./scripts/gate.py` is green.

## Progress

- Backlog. Filed while implementing `S-53` on 2026-08-08.

## Notes

`S-53` needed the browser kernel to refuse a coalesced frame, and the native WS transport had
already solved that problem — `parse_one` in `crates/sipx-transport/src/ws.rs`, whose own comment
says the quiet part: *"a second copy of those rules is a second place for them to drift"*. The
kernel could not call it, because `sipx-transport` reaches tokio and a socket and the kernel builds
for `wasm32-unknown-unknown` with neither. So `S-53` wrote the second copy that comment warns
about, deliberately and with the story's own guidance — *"a new public function on `sipx-sip` is a
wider change than this story needs"* — and this is that wider change, scoped on its own.

The two copies agree today, and the way they will stop agreeing is not by someone editing one of
them: it is by `StreamParser` gaining a rule that only one caller picks up. The `Content-Length`
fallback is the sharp edge. Both currently answer `FramingError::ContentLengthRequired` by falling
back to the datagram reading, which is what makes a body that looks like a second message one
message; a future framing error that also deserves that fallback would have to be added twice.

Nothing here should change behaviour. The value is entirely that the next reader finds one rule.
