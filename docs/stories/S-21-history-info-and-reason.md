---
id: S-21
title: Implement History-Info, and populate Reason
pillar: Signalling
status: done
priority: 11
design: docs/specs/sip-history-info.md
epic: conformance
areas: [sipx-sip, sipx-ua, sipx-call]
note: M11 · RFC 7044 + 3326 · who diverted a call and why; one story because 7044 §10.2 needs Reason
---

# Implement History-Info, and populate Reason

## Goal
Say what happened to a call on the way: which targets it was sent to, in what order, and why each
hop moved it. `History-Info` carries the where; `Reason` carries the why, and RFC 7044 puts the
second inside the first.

## Acceptance

**Reason (RFC 3326)**
- [x] `Reason` is populated rather than only preserved: on a CANCEL and on a BYE, with `protocol` of
      `SIP` or `Q.850` and the `cause` matching — §2: "SIP: The cause parameter contains a SIP status
      code. Q.850: The cause parameter contains an ITU-T Q.850 cause value in decimal representation."
- [x] The §3.1 case is expressible: cancelling the remaining branches of a forked request with the
      status code that won carried in the `Reason`. sipx does not fork, but a coupled call (`C-1`)
      cancelling its outbound leg because the inbound one went away is the same shape, and the peer
      deserves to know which it was.
- [x] `Reason` is placed only where §2 permits it: "in any request within a dialog, in any CANCEL
      request and in any response whose status code explicitly allows the presence of this header
      field." Not on an initial INVITE.

**History-Info (RFC 7044)**
- [x] `History-Info` is a typed list header (§5): `hi-entry` values of an `hi-targeted-to-uri` with
      `index`, and the target parameters `rc`, `mp` and `np` with the meanings §5 gives them — `rc` a
      Request-URI change for the same target user, `mp` a different target user, `np` no change.
- [x] Indexing follows §10.3: the first index "MUST be set to 1"; forwarding appends "the dot
      delimiter followed by an initial value of 1"; a visible gap gets "a single index with a value of
      '0' prior to adding the appropriate index".
- [x] `Supported: histinfo` (§6.1) is offered when the caller wants the history back in the response,
      and the history is returned in responses other than 100 (§9.3).
- [x] The `Reason` for a hop is carried where §10.2 puts it — "in the 'headers' component of the
      hi-targeted-to-uri in the last hi-entry added to the cache, unless the hi-targeted-to-uri is a
      Tel-URI" — which is why this is one story and not two.
- [x] Privacy is implemented, not skipped: a `Privacy` header field with `history` or `header` means
      the entries for the responsible domain are anonymized (§10.1). A diversion history that leaks a
      URI its owner asked to hide is worse than no diversion history.
- [x] The registry entries for RFC 7044 move off "not started" and RFC 3326 off `syntax only`, with
      `Roles` naming what sipx does — a UA that reads a received history and adds its own entry when
      it retargets, not a proxy that builds one for a fork.
- [x] Failing-first test: `a_retargeted_request_carries_the_previous_target_and_the_reason_it_moved`.

## Progress
- Added the normative contract in `docs/specs/sip-history-info.md`, including state/index tables and
  byte-level retarget, missing-hop, privacy, and call-reason vectors.
- Added typed `Reason` and `History-Info` values, URI-embedded reasons, first/forward/gap indexing,
  target-change references, tel-URI handling, and privacy-safe response generation in `sipx-sip`.
- Added the UA retarget operation and call integration: initial requests advertise and seed history,
  non-100 responses return it, established calls retain it, and CANCEL/BYE carry default or explicit
  typed causes. Focused SIP/UA and end-to-end CANCEL/BYE tests pass.
- RFC 3326 and RFC 7044 are now honest `partial` rows with UA roles, code evidence, limits, and the
  normative spec; `scripts/rfc-report.py --check` reports all 71 claims backed.
- The centrally coordinated 25-step workspace gate passed on the combined tree before closure.

## Notes
- The index is the part that looks like bookkeeping and is not. It is the only thing that says
  whether two entries are siblings from one retarget or a chain from several, and an implementation
  that appends a flat list is unusable for exactly the diagnosis the header exists for.
- Scope is the UA half. sipx does not fork, so §10.3's forking rules are implemented as far as
  reading a forked history correctly and no further; the entry-adding side is what a retargeting UA
  or a coupled call (`C-1`) does.
- Both halves are read by a downstream element before they are written by one, which is the argument
  for doing the parse and the privacy rules properly even though sipx generates the simpler cases.
