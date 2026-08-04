---
id: X-64
title: Pin the malformed-input refusals with named tests
pillar: Build
status: done
design: docs/designs/input-hardening.md
epic: input-hardening
areas: [sipx-sip, sipx-transport, beta4]
predicate:
announcement: 2
note: three recurring input classes · properties currently asserted by design, sampled by fuzzing, pinned by nothing · beta-1
---

# Pin the malformed-input refusals with named tests

## Goal

Convert three malformed-input properties sipx already holds from design claims into named tests
that fail if the property is removed, so a refactor cannot quietly restore a defect class that
recurs across independent SIP implementations.

## Acceptance

- [x] A request that frames correctly but omits the headers response construction reads
      (`To`, `From`, `Call-ID`, `CSeq`, the `Via` stack — RFC 3261 §8.2.6.1) yields a typed refusal
      from every public path that builds a response to it, proven by a failing-first test per path.
- [x] A table-driven test asserts the pre-allocation bound on **every** framing path independently —
      UDP datagram, TCP and TLS stream, WS and WSS frame (RFC 7118, RFC 6455 §5.2) — from one table,
      so a transport added later without the bound fails an existing test rather than needing a new
      one. The assertion is on the typed error and on a held bound, not on absence of a panic.
- [x] A declared length that disagrees with the bytes that follow — short body and long body — is a
      typed error or a bounded wait on every framing path; never a hang, never a read past the frame.
- [x] Each test carries its RFC citation in a comment on the test itself, and names the property it
      pins rather than the input it sends.
- [x] For each of the three classes, the story's Progress log records the scratch edit that removes
      the corresponding bound and the test failing against it. A test that passes with the bound
      removed does not satisfy this story.
- [x] `./scripts/gate.py` green.

## Progress
- Missing response headers exposed a real defect: `ResponseBuilder::to_request` built a response
  after silently omitting any absent `Via`, `From`, `To`, `Call-ID`, or `CSeq`. The new integration
  table checks both request validation and the response builder's typed refusal for each header.
  Scratch mutation: removing the builder guard made
  `cargo test -p sipx-sip --test input_hardening
  response_construction_refuses_each_missing_required_header_by_name` fail because the missing-`Via`
  row returned `Ok`.
- The SIP parser's body allocation bound already held on UDP, TCP, and TLS. Scratch mutation:
  replacing `check_body_limit` with `Ok(())` made
  `cargo test -p sipx-transport --all-features
  pre_allocation_body_and_frame_bounds_hold_on_every_framing_path --lib` fail on the UDP row, which
  returned `BodyTruncated` instead of the required `LimitKind::BodyBytes` refusal.
- WS and WSS exposed a real pre-allocation defect: their decoder used its independent 16 MiB frame
  and 64 MiB assembled-message defaults in front of sipx's configured limit. Both handshake paths
  now install the SIP limit as the WebSocket frame and message limits. Replacing that configuration
  with the decoder default made the same all-path bound test fail: the WS row observed 16,777,216
  bytes instead of its configured 256-byte held bound. The follow-up proof performs real client and
  server handshakes in both WS and WSS URI modes, then observes the installed decoder configuration
  and its typed refusal of an oversized frame before SIP parsing. Independently replacing the
  client and server handshake configuration arguments with `None` made
  `client_handshake_holds_the_frame_bound_for_ws_and_wss` and
  `server_handshake_holds_the_frame_bound_for_ws_and_wss` fail at 16,777,216 versus the configured
  32 bytes (exit 101). The framing table now uses exhaustive matches over production's
  `TransportKind`, rather than a private look-alike enum, so a new transport forces a compile-time
  framing decision.
- Endpoint-level propagation is pinned separately from those handshake-helper tests. The WS and
  WSS integration tests set `Config::limits` just above the normal one-megabyte stream ceiling,
  then exchange both a request and a response above that normal ceiling but below the configured
  one. The large request pins the inbound `accept_with_limits` path and the large response pins the
  outbound `connect_with_limits` path for each of WS and WSS: substituting `Limits::stream()` at
  any of those four boundaries makes the WebSocket decoder close before the custom SIP parser can
  accept the message. Focused runs of
  `custom_endpoint_limits_reach_inbound_and_outbound_ws_handshakes` and
  `custom_endpoint_limits_reach_inbound_and_outbound_wss_handshakes` both pass under the custom
  endpoint policy.
- Short and long declared-body disagreements are covered from the same UDP/TCP/TLS/WS/WSS table.
  Scratch mutations proved both sides: bypassing the stream short-body wait made
  `cargo test -p sipx-transport --all-features
  body_length_disagreement_is_typed_or_bounded_on_every_framing_path --lib` panic while splitting
  four bytes from three; accepting WS trailing bytes made that test fail because the long-body row
  accepted bytes beyond `Content-Length`.
- Targeted evidence: `cargo test -p sipx-sip -p sipx-transport --all-features` passes (238 sipx-sip
  unit tests, all integration/property/corpus tests, 163 sipx-transport unit tests, and all transport
  integration tests). The full repository gate remains outstanding for release integration.

## Notes
- The properties already hold: bounds are checked before allocation at
  `crates/sipx-sip/src/parser.rs:18-35`, with 64 KiB datagram and 1 MiB stream profiles. This story
  adds no defence; it adds the tests that keep them.
- `crates/sipx-testkit`'s in-process link drives the datagram and stream paths without sockets. Whether
  WS and WSS can be driven at the same level, or need a per-transport test sharing assertions through
  a helper, is the design's open question — resolve it in the story, do not skip the paths.
- Not a fuzzing story. `fuzz/fuzz_targets/` already samples unknown inputs; this pins known properties.
  See [`docs/designs/input-hardening.md`](../designs/input-hardening.md) for why they are not substitutes.
