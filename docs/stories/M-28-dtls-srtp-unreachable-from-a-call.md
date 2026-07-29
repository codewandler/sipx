---
id: M-28
title: Offer DTLS-SRTP from a call, and stop claiming it until then
pillar: Media
status: in-progress
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
      *(Not done. The three lines are now 1708, 2839 and 3141 after `M-29`. `docs/specs/srtp.md`
      §12.8 states what reaching keys needs and why it is not one row of wiring.)*
- [ ] **Which keying a call uses is the application's choice, with a stated default.** SDES puts
      the master key in the SDP body and so requires secure signalling (RFC 4568 §7.1); DTLS-SRTP
      does not. That difference is the reason to expose the choice rather than infer it.
      *(Not done in code. The rule is now normative in `docs/specs/srtp.md` §12.8, which is where
      the next attempt should start; no selector was added, because one that cannot reach keys
      would offer a token this side cannot honour.)*
- [x] **The registry stops over-claiming, whatever happens to the code.** RFC 5763 and RFC 5764 are
      `status = "implemented"` with `roles = ["uac", "uas"]`, and `with_dtls_srtp` has no caller
      outside `sipx-sdp`'s own test module. A reader of `docs/compliance.md` would conclude sipx
      places DTLS-SRTP calls. It cannot. Either this story makes the claim true, or the rows say
      which half is missing — **the rows get corrected either way, and that half is not optional.**
      → both rows are `partial` with no roles listed and a note leading with the missing half
      (`docs/rfc/registry.toml`), `docs/compliance.md` regenerated, `docs/specs/srtp.md` §12.8.
- [ ] `X-27`'s interop harness runs the DTLS-SRTP case once there is a sipx side to exercise:
      name the test in `KEYING_TESTS[dtls]` and drop the `KEYING_UNIMPLEMENTED[dtls]` entry. The
      harness is already shaped for it. *(Not done, and correctly so: there is still no sipx side.
      `tests/interop/run.sh`'s `KEYING_UNIMPLEMENTED[dtls]` text remains accurate as written.)*
- [ ] Failing-first test: a call placed with DTLS-SRTP selected offers `UDP/TLS/RTP/SAVP` and a
      fingerprint. It cannot pass today, because the option does not exist. *(Not written. No code
      changed, so there was nothing to falsify; writing the test without the selector would have
      left a test asserting the gap rather than closing it.)*

## Progress
- **2026-07-29 — registry corrected, code half deliberately not attempted.** The half the story
  called non-optional is done: RFC 5763 and RFC 5764 are `partial`, list no roles, and each note
  opens with "Missing: a call. No role is reachable from `sipx-call`". `docs/compliance.md` is
  regenerated and `./scripts/rfc-report.py --check` is green.
- **Verified against the current code, not the story text.** `M-29` moved the lines the Acceptance
  cites; the three paths are `crates/sipx-call/src/call.rs:1708` (`offered_media`, the offerer),
  `:2839` (`Early::settle`, the early answer) and `:3141` (`answer_negotiated`). All three call
  `Capabilities::with_srtp`. `srtp_keys` is now the RFC 4568 §5.1.3 check and `srtp_keys_answering`
  its answering counterpart; a keying selector belongs *above* both and must not weaken either.
- **What the answering side already does right, so nobody re-fixes it:** `sipx_sdp::answer` rejects
  a `UDP/TLS/RTP/SAVP` stream outright when `capabilities.dtls` is `None`
  (`crates/sipx-sdp/src/answer.rs:231-238`), and `answer_negotiated` turns an all-rejected answer
  into `Error::NoCommonCodec` (`call.rs:3143`). So a DTLS offer to sipx is declined today rather
  than answered in the clear — §7 rule 4 holds, and no plain media session is started behind it.
- **Why no selector was landed.** Four obstacles, all in `docs/specs/srtp.md` §12.8. The decisive
  one is ordering: `dial_with` calls `establish` *before* the ACK, under a stated invariant that
  every path from a 2xx must acknowledge. A handshake inside `establish` holds the ACK, and a peer
  that starts DTLS only after the ACK deadlocks with it until the handshake times out. Keying has
  to move after the acknowledgement, which reshapes the 2xx path rather than adding to it — and
  the same lending of the media socket is needed at five separate start sites.
- **Next step for whoever picks this up:** start from §12.8's numbered list, and take the ACK
  ordering first — it decides the shape of everything after it. `dtls::openssl::Identity::generate`
  and `dtls::establish` are ready; nothing in `sipx-sdp` needs changing for either direction.

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
