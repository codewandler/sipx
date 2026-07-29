---
id: X-17
title: Interoperate against a second independent implementation
pillar: Build
status: backlog
priority:
design:
epic: conformance
areas: [sipx-testkit, build]
note: M12 · one interop peer is a sample of one, and no peer has ever answered a sipx call
---

# Interoperate against a second independent implementation

## Goal
Prove sipx against more than one implementation it did not write, and against a *user agent* as well
as a server — because today no independent implementation has ever answered a call sipx placed.

## Acceptance
- [ ] `tests/interop/run.sh` is parameterised over the peer instead of hardcoding one image, one
      container name and one configuration directory. Adding a peer must be adding a profile, not
      editing the script.
- [ ] A second **server** peer runs the same test list as the first, unchanged. The test list is the
      contract; if a test needs rewording per peer, that wording was hiding an assumption.
- [ ] A second peer is chosen by criteria recorded in `tests/interop/README.md`, not by preference:
      an independent lineage from the first, scriptable without interaction, obtainable in CI, and a
      licence that permits it. The point of a second peer is that it shares no code and no reading of
      the RFCs with the first.
- [ ] A **user agent** peer answers a call sipx places and places one sipx answers, with SDP
      negotiated, audio flowing and a BYE ending it. This is the gap that matters most: RFC 3264 is
      recorded as `implemented`, and every offer/answer test in the repo has sipx on both sides.
      `M-1`'s pure function has never met a foreign answerer.
- [ ] The media assertion is real, not "a session was set up": audio sipx sends is received, and
      audio the peer sends arrives with the payload type the negotiation chose. `M-3`'s bit-exactness
      check is the precedent, relaxed only as far as a foreign encoder forces.
- [ ] The whole matrix runs in CI on the same `#[ignore]`d-by-default discipline, so `cargo test`
      still needs no containers.
- [ ] Whatever the new peer disagrees with sipx about is filed as its own story with the RFC sentence
      that settles it — the way `X-6` handled the first round of conformance defects. This story is
      the measurement, not the fixing.
- [ ] Failing-first test: `an_independent_user_agent_answers_a_call_sipx_placed`.

## Progress
- Implemented. `tests/interop/run.sh` no longer names an image, a container or a configuration
  directory: a peer is a directory beside it holding a `profile.sh`, and `run.sh --list` is what
  CI's matrix is built from. Both peers were run end to end and both are green.
- The second peer is Asterisk 20 (`andrius/asterisk:20.20.1-alpine-3.24`), chosen against the
  criteria now recorded in `tests/interop/README.md`. Its SIP handling is PJSIP, which shares no
  ancestry with the first peer's own parser — so a message leaving sipx is now read by two
  parsers with no common code.
- The eight non-WebSocket server tests passed against the second peer unchanged, first attempt.
  `run.sh` owns the test list; a profile declares only which *roles* it can play (`server`,
  `user-agent`), because a proxy has no dialplan and cannot answer a call.
- `an_independent_user_agent_answers_a_call_sipx_placed` and its mirror
  `an_independent_user_agent_places_a_call_sipx_answers` live in
  `crates/sipx-cli/tests/interop_call.rs`. The media assertion runs in relay mode and compares
  the µ-law bytes the peer echoed against the µ-law bytes sipx sent: the whole 600 ms clip comes
  back byte for byte in both directions. Confirmed non-vacuous against an extension that answers
  and stays silent — it fails on "a session was set up and nothing was heard".
- One divergence found and filed as `T-23`: sipx's WebSocket client hardcodes the request path
  `/`, and the second peer serves SIP over WebSocket at `/ws` on its HTTP server's own port.
  RFC 7118 §5 fixes neither, so this is a gap in what a `Target` can express. Recorded in the
  profile's `PEER_DIVERGES_ON` and announced on every run rather than reworded into the test.
- Two peer defaults cost time and are written down in the README's traps: the second peer's
  `res_pjsip` defaults to TLS 1.0, which current OpenSSL refuses outright (`openssl s_client
  -tls1_2` fails against it too) — the same shape of trap the first peer sprang with its pinned
  TLS 1.2 method.

## Notes
- The interop README already states the reason this story exists better than a story can:
  "if the parser and the builder both misread the same sentence of RFC 3261, they agree perfectly
  and interoperate with nothing." One peer narrows that risk; it does not close it.
- Expect the UA half to be where the findings are. Registration is a narrow, well-trodden exchange;
  offer/answer has many more places for two readings of RFC 3264 to differ, and every one of them is
  a call that connects to silence rather than a call that fails.
- A peer that needs a GUI or a manual step is not a peer for this purpose, however good it is. The
  [vision](../vision.md)'s "testable from a shell" applies to the harness too.
