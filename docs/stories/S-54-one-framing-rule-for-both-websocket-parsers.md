---
id: S-54
title: One framing rule for both WebSocket message parsers
pillar: Session
status: done
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

- [x] `crates/sipx-transport/src/ws.rs`'s `parse_one` and `crates/sipx-wasm/src/kernel/mod.rs`'s
      `parse_frame` are one function, in `sipx-sip`, reachable with default features off so the
      `wasm32-unknown-unknown` build still resolves no entropy source, clock or socket.
- [x] It is public API on a published crate, so it carries rustdoc saying what it promises — one
      whole message and nothing else — and what it does not: it is not the datagram tolerance of
      RFC 3261 §18.3, and it does not require `Content-Length`.
- [x] A differential test drives both callers over the same frames, including the two shapes that
      separate this rule from `parse_datagram` (two complete messages; one message plus trailing
      octets) and the two that separate it from the stream framer (no `Content-Length`; a leading
      CRLF keep-alive).
- [x] Neither caller's observable behaviour changes: `sipx-wasm`'s corpus replay digest and
      `sipx-transport`'s WS framing cases are unchanged, and the change is visible only as a diff
      that deletes one of the two copies.
- [x] `./scripts/gate.py` is green.

## Progress

- Backlog. Filed while implementing `S-53` on 2026-08-08.
- 2026-08-09 — **the rule was lifted, not documented as a pair.** The Notes' question was judged
  again with the duplication in front of it and the answer went the other way from `S-53`'s: the
  rule is now `sipx_sip::parse_frame`, in `crates/sipx-sip/src/parser.rs` beside `parse_datagram`
  and `StreamParser`, and both copies are deleted. It belongs there because the difference between
  the three readings is one sentence — *what a leftover means* — and that sentence was being
  written out in three places instead of once. `sipx-sip` reaches no runtime, socket or clock, so
  it is the only home both callers can see; `check-wasm-kernel.sh` confirms the
  `wasm32-unknown-unknown` build still resolves none of them.
- **What keeps the enforcers in step** is now two things rather than a hope. They are one function,
  so there is no second implementation to drift. And `docs/specs/sip-tls.md` §6.1 is a table of
  seven frames with their verdicts, read out of the spec at compile time by
  `ws::tests::every_spec_framing_vector_holds_for_the_websocket_transport` and by
  `sipx-wasm`'s `every_spec_framing_vector_holds_for_the_kernel` — so a caller that re-acquires a
  copy of the rule fails the row it disagrees with. Both tests additionally require the four shapes
  that separate this rule from its neighbours to be *present* in the table, because a table that
  quietly lost one would still be a table.
- **Failing-first, at the merge base `c405a60`.** `every_spec_framing_vector_holds_for_the_kernel`
  failed with `docs/specs/sip-tls.md §6.1 has no 'two-complete-messages' vector; found []`, and the
  transport's twin did not compile: `error[E0425]: cannot find function 'parse_frame' in this
  scope`. Between them that is the story — no shared statement of the rule, and no shared home for
  it.
- **Proved it can fail.** With the shared rule in place, the kernel was given a second copy of it
  that disagrees (`StreamParser::push(..).ok().and_then(|mut m| m.pop())`, the pre-`S-53`
  reading). `every_spec_framing_vector_holds_for_the_kernel` failed on `two-complete-messages`
  (`§6.1 refuses this frame … left: 0, right: 1`) while the transport's twin still passed — one
  side disagreeing, caught, from one corpus. Reverted; the file's SHA-256 was compared before and
  after to be sure nothing of it survived.
- **The pinned digest did not move.** `the_corpus_replay_digest_is_stable_across_targets` passes
  unchanged at `32c86355…a42e`, natively and under `wasm32-wasip1`, so no re-derivation was needed
  and none was done. `S-53`'s comment above the constant — re-derived once, for `dblreq.dat`, and
  for nothing else — is still true.
- **CHANGELOG sentence owed** (fenced file, not edited here): *`sipx_sip::parse_frame` parses the
  one SIP message a self-delimiting frame carries, refusing a frame that holds anything else with
  the new `FramingError::NotExactlyOneMessage`. It is the single implementation of RFC 7118 §5's
  framing rule, which the WebSocket transport and the browser kernel each used to carry a copy
  of.* Additive on a published crate: `FramingError` is `#[non_exhaustive]`, so a downstream match
  already carries a `_` arm.
- Gate not run — one gate per wave, by the coordinator. Verified here: `cargo test` and `cargo
  clippy -D warnings` on `sipx-sip`/`sipx-wasm`/`sipx-transport` `--all-features`, `cargo fmt
  --all`, a workspace-wide `cargo check --all-targets --all-features` because the change touches a
  published error enum, `check-wasm-kernel.sh`, `check-browser-binding.sh`,
  `check-provenance.sh`, `check-docs-links.py` and `check-app-surface.py --check`.

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

- 2026-08-09: closed at the `1.0.0-rc.13` boundary, against the wave gate run on this tree.
