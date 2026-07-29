---
id: M-28
title: Offer DTLS-SRTP from a call, and stop claiming it until then
pillar: Media
status: ready
priority: 5
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
- **The gap is wider than this story and `docs/compliance.md:107-109` both record**, found by a
  read-only public-docs sweep and worth knowing before anyone estimates the work. The registry row
  says no role is reachable from `sipx-call`; in fact **no `MediaSession` can be keyed by DTLS at
  all**, on two independent boundaries:
  1. **The types never meet.** Everything that turns a handshake into keys returns pre-built SRTP
     contexts — `dtls::Keys { outbound: srtp::Context, inbound: srtp::Context }`
     (`sipx-media/src/dtls/mod.rs:116-121`), returned by `establish` (`:240`) and
     `keys_from_exported` (`:150`). Nothing in `sipx-media`'s public surface accepts an
     `srtp::Context`: `Config.srtp` is `Option<SrtpKeys>` (`session.rs:264`), and `SrtpKeys`
     (`:375-381`) is master key *and salt* per direction, converted internally at `:1509-1511` via
     `SrtpContext::new(key, salt)`. So this is not one row of wiring; it is a missing constructor.
  2. **The handshake cannot run on the port RFC 5764 §5.1.2 requires it to share.**
     `MediaPort.socket` is a private tokio `UdpSocket` with no accessor (`session.rs:801-810`),
     while `dtls/openssl.rs:165-169` needs an owned `std::net::UdpSocket` it `connect`s itself.
- **Consequence for the docs, now `X-35`'s**: `website/docs/intro.md:43-45` and
  `whats-new.md:36-38` tell readers DTLS-SRTP is "reachable by building your own capabilities with
  `sipx-sdp` and `sipx-media`". That workaround cannot be written. A determined user could implement
  `Handshake`, call `export(60)` and re-split per §4.2 by hand into `SrtpKeys`' public fields — but
  that bypasses `establish`'s RFC 8122 §6.2 fingerprint check and still cannot share the media port.
- **2026-07-29 — the docs half above is done, by `X-35`.** `website/docs/intro.md`,
  `whats-new.md` and `does-this-fit.md` no longer offer the workaround that cannot be written;
  all three now say no media session can be keyed by DTLS today by any route, name both
  boundaries, and point here. Nothing about the code changed, and the Acceptance items below are
  untouched — this only removes the reader-facing claim that the work was already reachable.
- Also: `capabilities.dtls()` is *read* at `crates/sipx-call/src/call.rs:3600` and **nothing anywhere
  ever sets it**. The branch exists and is dead, which is the shape RFC 8122 had before it was
  demoted — a reachability check that only follows evidence paths cannot see it.
- **Still open, and deliberately.** One of five Acceptance items is done — the registry
  correction, which the story made unconditional. The code half remains and is the reason the
  story keeps its file rather than closing: `M-28` is not "DTLS-SRTP paperwork", it is "offer
  DTLS-SRTP from a call". Re-prioritised 2 → 5, because the over-claim that made it urgent is gone
  and what is left is a feature rather than a correction.
- **Adjacent, not done:** RFC 8122's row has the same shape — `status = "implemented"`, both roles,
  and `a=fingerprint` is never emitted or read by any call for exactly the same reason. It was left
  alone because the Acceptance names 5763 and 5764. Whoever closes the code half should correct it
  in the same commit, or it becomes the fifth instance of the pattern.
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
