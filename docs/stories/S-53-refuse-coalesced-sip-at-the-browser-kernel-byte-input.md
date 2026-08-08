---
id: S-53
title: Refuse coalesced SIP at the browser kernel's byte input
pillar: Session
status: backlog
priority: 30
design: docs/specs/browser-signalling.md
epic: browser-sdk
areas: [wasm, websocket, browser, parsing]
predicate:
announcement:
note: found by T-33 · sip-tls.md §4 says close, the kernel silently truncates
---

# Refuse coalesced SIP at the browser kernel's byte input

## Goal

Make `sipx_input_bytes` refuse a WebSocket message that carries more than one SIP message, so the
browser kernel enforces the framing rule `docs/specs/sip-tls.md` §4 already states and the native
WebSocket transport already keeps.

## Acceptance

- [ ] A `sipx_input_bytes` buffer holding two complete SIP messages is refused: `parse_errors`
      increments, no records are produced, and no transaction, dialog or response is created from
      the first of them. A failing-first test asserts the current behaviour is gone.
- [ ] A buffer holding one SIP message followed by trailing octets that are not a second message is
      refused on the same terms — the rule is "the frame is the message", not "the frame starts
      with a message".
- [ ] A buffer holding exactly one SIP message, with and without a body, is unaffected; the
      RFC 4475 and RFC 5118 replay and the pinned wire digest are unchanged.
- [ ] `docs/specs/browser-sdk.md` §4.3's `sipx_input_bytes` row, or a subsection under it, states
      the one-message rule and cites `docs/specs/sip-tls.md` §4.
- [ ] `browser/test/kernel.test.mjs`'s "a coalesced frame is neither split nor answered by the
      binding" case is tightened to assert the connection closes with `framing`, and the paragraph
      in `docs/specs/browser-signalling.md` §5 naming this story is removed.

## Progress

- Backlog. Found while implementing `T-33` on 2026-08-08.

## Notes

Measured against the compiled `sipx_browser_wasm.wasm` at `18a1c4d`, driving `sipx_input_bytes`
directly:

| Input | `parse_errors` | records |
|---|---|---|
| `SIP/2.0 100 Trying\r\n\r\nSIP/2.0 200 OK\r\n\r\n` | unchanged | none |
| two complete OPTIONS requests in one buffer | unchanged | a WIRE response and a TIMER_SET |
| half a REGISTER | +1 | none |
| garbage | +1 | none |

So a fragment is refused correctly and a **coalesced** frame is not: the kernel parses the first
message, acts on it, and discards the rest with no counter and no event. `docs/specs/sip-tls.md`
§4 is explicit that both are malformed and that both close the connection, "because a peer that
frames wrongly has revealed it disagrees about where messages end, so nothing further from it can
be trusted to be what it claims".

The cause is `Kernel::input_bytes` in `crates/sipx-wasm/src/kernel/mod.rs`, which calls
`sipx_sip::parse_datagram` and treats a successful parse as covering the whole buffer. Datagram
semantics are right for UDP, where the frame really is the datagram; over WebSocket the same
tolerance silently drops a message.

`T-33`'s binding cannot close this from the host side and deliberately does not try: it has no way
to tell a coalesced frame from a well-formed one without parsing SIP in JavaScript, which
`docs/specs/browser-sdk.md` §8.1 concentrates in the kernel on purpose. The fix belongs where the
parser is.

The likely shape is for the parser to report how many octets it consumed, so the kernel can compare
against the buffer length — check whether `sipx_sip` already exposes that before adding it, since a
new public function on `sipx-sip` is a wider change than this story needs.
