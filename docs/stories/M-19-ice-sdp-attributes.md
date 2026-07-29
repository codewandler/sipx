---
id: M-19
title: Parse and serialise the ICE attributes in SDP
pillar: Media
status: in-progress
priority: 7
design: docs/designs/media.md
epic: ice
areas: [sipx-sdp]
note: ice · RFC 8839 §5 · pure parsing; nothing else can negotiate until this exists
---

# Parse and serialise the ICE attributes in SDP

## Goal
Parse and serialise every attribute RFC 8839 defines, so the rest of ICE negotiates over
a typed description rather than a substring search. Pure parsing: no clock, no socket, no runtime.

## Acceptance
- [x] `candidate` (§5.1) parses and serialises: the RFC's own example line round-trips byte-identically,
      including `raddr`/`rport`, and unknown `cand-extension` pairs survive rather than being rejected.
- [x] `ice-ufrag`/`ice-pwd` (§5.4) at session and media level with **media level winning**; ≤32 chars
      on send and ≤256 accepted on receive.
- [x] `ice-options` (§5.6), `ice-lite` and `ice-mismatch` (§5.3) at their stated levels,
      `remote-candidates` (§5.2), `ice-pacing` (§5.5).
- [x] `priority` is range-checked to 1..=2^31−1 on parse. The grammar is `1*10DIGIT`, so `4294967295`
      parses, and the §6.1.2.3 pair-priority arithmetic overflows `u64` on it — the check is what
      makes that arithmetic safe (spec §4, §6.2).
- [x] A `candidate` line with an FQDN or an unsupported address family is **ignored** and the rest of
      the description survives; a transport other than UDP is accepted and discarded (spec §3).
- [x] Failing-first test: `the_rfc_8839_candidate_example_round_trips_unchanged`.
- [x] No new dependency, and `sipx-sdp` gains no runtime, socket or clock read.

## Progress
- Done, on `impl/M-19`. The grammar is [`sipx_sdp::ice`](../../crates/sipx-sdp/src/ice.rs); the
  accessors that apply RFC 8839 §5's per-attribute levels are on `MediaDescription` and
  `SessionDescription` in `session.rs`. 17 tests, all derived from the RFC's own example lines and
  `docs/specs/ice.md` §14's vectors. `sipx-sdp/Cargo.toml` is untouched: no new dependency, and
  nothing in the module reads a clock, opens a socket or names a runtime.
- Three judgements the next reader should know were made deliberately, and why:
  - **`raddr`/`rport` are not enforced on receive.** §5.1 requires them for `srflx`/`prflx`/`relay`
    and forbids them for `host`, and a candidate sipx *generates* obeys that. Reading is lenient:
    §5.1 gives the field to diagnostics, nothing in RFC 8445's checks consults it, and dropping a
    peer's only working candidate over a diagnostic field trades a call for a nicety. What is
    enforced is that a *half* pair — `raddr` with no `rport` — is malformed, because the
    alternative is inventing the other half into a description sipx may relay.
  - **`ComponentId` is a bounded number, not an `Rtp`/`Rtcp` enum.** §5.1 makes it 1–256. The spec's
    §3 table names 1 and 2, and they are consts on the type; refusing component 3 would drop a line
    the peer is entitled to send.
  - **`Candidate` carries no `base`.** The spec's §3 table lists one; it is a gathering-side fact
    that never appears on the wire, so including it here would put a field in the type that a round
    trip cannot fill. It belongs on `sipx-media`'s candidate, in the story that gathers.
- **The registry rows for RFC 8839 were left to `M-22`**, as dispatched. `docs/rfc/registry.toml` is
  therefore silent about this crate's newest grammar until then, and `rfc-report.py --check` stays
  green because it verifies the claims that are made, not the ones that are missing.
- **`docs/specs/ice.md` §6.2 is corrected, in this branch.** It said the largest pair priority "is
  `2^63 − 1`" and implied that anything past 2^31 − 1 overflows. Both were wrong; the warning they
  supported was not. `2^63 − 1` is a bound approached and never reached — the `(G>D?1:0)` term is
  zero exactly when the operands are equal, so the attained maximum is `2^63 − 2` — and the
  overflow needs `u32::MAX` on both sides, not one step past the range. Two contexts computed this
  independently before it was changed. The Acceptance above (the `priority` item) repeats the old
  framing and was left as written, because the Acceptance is the contract as dispatched and the
  check it asks for is the check that was built; this note is the record of which text was wrong.
  Asserted by `the_priority_bound_is_what_keeps_the_pair_priority_in_a_u64`, and the module's own
  rustdoc now says the same thing as the test.
- **The "candidates vanish silently" risk was measured and is smaller than reported.** A peer whose
  candidates are all ignored is *not* indistinguishable from a peer doing no ICE:
  `ice_credentials_for()` still returns `Some`, and the raw `a=candidate` lines are still on the
  description and still countable. That is what a consumer needs for `ice-mismatch` (§13.2) and for
  §13.3's fallback. What is missing is a named API saying so, and it belongs to `M-21`/`M-22`
  rather than here.
- An empty `cand-extension` value — which `extension-att-value = *VCHAR` admits — drops the
  candidate rather than being kept. Keeping it would put a trailing space on every round trip,
  because an empty value is only distinguishable from a missing one by that space, and a round
  trip that adds a byte the peer did not send is the worse failure. Pinned by
  `a_malformed_candidate_is_ignored_rather_than_fatal`.

## Notes
- The spec is [`docs/specs/ice.md`](../specs/ice.md), written by `M-16` before any code. Read the
  sections its Acceptance names rather than re-deriving them from the RFCs.
- `M-16` is the tracker for this epic and stays open until every child is done.
