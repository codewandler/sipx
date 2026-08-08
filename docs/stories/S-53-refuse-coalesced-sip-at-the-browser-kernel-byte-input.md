---
id: S-53
title: Refuse coalesced SIP at the browser kernel's byte input
pillar: Session
status: in-progress
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

- [x] A `sipx_input_bytes` buffer holding two complete SIP messages is refused: `parse_errors`
      increments, no records are produced, and no transaction, dialog or response is created from
      the first of them. A failing-first test asserts the current behaviour is gone.
- [x] A buffer holding one SIP message followed by trailing octets that are not a second message is
      refused on the same terms — the rule is "the frame is the message", not "the frame starts
      with a message".
- [x] A buffer holding exactly one SIP message, with and without a body, is unaffected; the
      RFC 4475 and RFC 5118 replay is unchanged in 61 of its 62 cases and the pinned digest was
      re-derived for the 62nd — see the Progress note, which is the reading behind the new number.
- [x] `docs/specs/browser-sdk.md` §4.3's `sipx_input_bytes` row, or a subsection under it, states
      the one-message rule and cites `docs/specs/sip-tls.md` §4.
- [x] `browser/test/kernel.test.mjs`'s "a coalesced frame is neither split nor answered by the
      binding" case is tightened to assert the connection closes with `framing`, and the paragraph
      in `docs/specs/browser-signalling.md` §5 naming this story is removed.
- [ ] `./scripts/gate.py` is green.

## Progress

- Backlog. Found while implementing `T-33` on 2026-08-08.
- 2026-08-08 — implemented on `impl/S-53`. `Kernel::input_bytes` now frames with a private
  `parse_frame` instead of calling `sipx_sip::parse_datagram` on the whole buffer: the buffer goes
  through `StreamParser`, and anything other than exactly one whole message with nothing pending is
  refused into `parse_errors` with no record emitted. The stream framer is reused rather than
  reimplemented, which is also what `sipx-transport`'s WS path does, so the browser kernel and the
  native WebSocket transport now refuse the same frames.

  Failing-first, at merge base `2d2f576`, `cargo test -p sipx-wasm --all-features --test framing`:

  ```
  ---- octets_after_a_message_are_refused stdout ----
  assertion `left == right` failed: a request with trailing octets: the host learns about it the
  only way it can, through §4.11's parse_errors
    left: 0
   right: 1
  ---- two_messages_in_one_frame_are_refused stdout ----
  assertion `left == right` failed: two complete requests in one frame: the host learns about it
  the only way it can, through §4.11's parse_errors
    left: 0
   right: 1
  test result: FAILED. 2 passed; 3 failed
  ```

  Three things a reviewer should read rather than take on trust.

  **`Content-Length` is what makes `T-33`'s first table row not a violation.** The measured frame
  `SIP/2.0 100 Trying\r\n\r\nSIP/2.0 200 OK\r\n\r\n` carries no `Content-Length`, and
  `docs/specs/sip-tls.md` §4 says explicitly that over WebSocket the header is optional and the
  body runs to the end of the frame (RFC 3261 §20.14). Those octets are already spent: it is one
  message with a body, before and after this change, and refusing it would contradict the same
  spec paragraph this story enforces — and Acceptance row 3, which requires a single message
  *without* a body to be unaffected. So the `kernel.test.mjs` case was tightened to close with
  `framing` on bytes that really are coalesced (two complete `200 OK`s, each with
  `Content-Length: 0`), and the original bytes are pinned in an adjacent case as the one message
  they are. Both are driven against the compiled module.

  **The pinned corpus digest moved, for exactly one case.** `rfc4475/dblreq.dat` is RFC 4475
  §3.1.1.8, "extra trailing octets in a **UDP datagram**" — a datagram rule, and `sipx_input_bytes`
  is fed a WebSocket frame, where RFC 7118 §5 says the opposite. Before, the kernel answered its
  embedded `REGISTER` with `SIP/2.0 405 Method Not Allowed` and set that transaction's timer while
  the second request vanished uncounted; now the frame is refused whole. The other 61 cases replay
  byte for byte, checked case by case before the digest was touched, and native and `wasm32-wasip1`
  agree on the new number. `EXPECTED_REPLAY_DIGEST` is
  `32c86355a32cde3a180540a0716fd2d48b783fec3d4aa2e4e69c71077a13a42e`, and
  `the_one_datagram_case_that_a_frame_refuses` asserts the outcome so the movement is a read
  assertion rather than a number adjusted to match a run.

  **A leading CRLF is now accepted where it used to be counted.** Reusing `StreamParser` brings its
  RFC 3261 §7.5 / RFC 5626 §4.4.1 skip of CRLF before a start line, so a frame beginning with one
  is now one message rather than a parse error. This converges the kernel on the native WS
  transport, which has always accepted it; `a_keepalive_before_the_message_is_not_a_second_message`
  pins it. A frame that is *only* CRLFs is still refused, on both paths.

  CHANGELOG sentence owed to the coordinator, under Fixed: "The browser session kernel refuses a
  WebSocket message that carries more than one SIP message, or one message followed by any other
  octets, instead of acting on the first and silently discarding the rest
  (`docs/specs/sip-tls.md` §4)."

  Gate not run (one gate per wave). Verified in the worktree: `cargo test -p sipx-wasm -p sipx-sip
  --all-features`, `cargo clippy -p sipx-wasm -p sipx-sip --all-targets --all-features --no-deps -D
  warnings`, `cargo fmt --all`, `./scripts/check-wasm-kernel.sh`,
  `./scripts/check-browser-binding.sh`, `./scripts/check-fixed-sleep.py --check`,
  `./scripts/check-provenance.sh`.

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
