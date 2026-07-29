---
id: M-28
title: Offer DTLS-SRTP from a call, and stop claiming it until then
pillar: Media
status: ready
priority: 2
design: docs/specs/srtp.md
epic: media
areas: [sipx-call, sipx-media, docs]
note: found by X-27 — dial hardcodes SDES, so sipx cannot offer DTLS-SRTP at all, while RFC 5763 and 5764 are both marked implemented with both roles
---

# Offer DTLS-SRTP from a call, and stop claiming it until then

## Goal
Make DTLS-SRTP reachable from `sipx-call`, so the keying `M-15` built can actually be used by a
call — and until it is, make the compliance table say so instead of claiming both roles.

## Acceptance
- [ ] A call can offer and answer `UDP/TLS/RTP/SAVP` with `a=fingerprint`. Today every offer path
      in `sipx-call` hardcodes `.with_srtp(transport.is_secure())` — `crates/sipx-call/src/call.rs`
      lines 1708, 2199 and 2501 — which is SDES, and `DialOptions` has no keying selector at all.
- [ ] **Which keying a call uses is the application's choice, with a stated default.** SDES puts
      the master key in the SDP body and so requires secure signalling (RFC 4568 §7.1); DTLS-SRTP
      does not. That difference is the reason to expose the choice rather than infer it.
- [ ] **The registry stops over-claiming, whatever happens to the code.** RFC 5763 and RFC 5764 are
      `status = "implemented"` with `roles = ["uac", "uas"]`, and `with_dtls_srtp` has no caller
      outside `sipx-sdp`'s own test module. A reader of `docs/compliance.md` would conclude sipx
      places DTLS-SRTP calls. It cannot. Either this story makes the claim true, or the rows say
      which half is missing — **the rows get corrected either way, and that half is not optional.**
- [ ] `X-27`'s interop harness runs the DTLS-SRTP case once there is a sipx side to exercise:
      name the test in `KEYING_TESTS[dtls]` and drop the `KEYING_UNIMPLEMENTED[dtls]` entry. The
      harness is already shaped for it.
- [ ] Failing-first test: a call placed with DTLS-SRTP selected offers `UDP/TLS/RTP/SAVP` and a
      fingerprint. It cannot pass today, because the option does not exist.

## Progress
- Not started.

## Notes
- Found by `X-27` while wiring encrypted-media interop. It could deliver the SDES half against a
  real peer and stopped at DTLS-SRTP because there is **no sipx side to test** — correctly refusing
  to edit library code outside its fence rather than improvising one.
- **This is the third instance of one pattern in two days**, and that is the finding worth carrying
  forward: `M-22` built ICE that no call can offer (`M-27`), RFC 3311 claimed an UPDATE role the
  caller cannot reach (`S-22`), and here a keying that is implemented, tested, marked `implemented`
  for both roles, and unreachable. **A crate-level capability with no caller at the call layer
  reads as a shipped feature in the compliance table.** Worth asking, once, whether the registry
  should distinguish "implemented in a crate" from "reachable from a call" — that is a candidate
  `X` story of its own, not this one.
- `M-25` is the cautionary sibling: a claim that nothing exercised end to end was wrong for six
  releases. Unreachable code is untested code with better paperwork.
