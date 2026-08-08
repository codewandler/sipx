---
id: M-81
title: Make the SRTP protect-error discard tell the truth
pillar: Media
status: done
priority: 6
design:
epic: media
areas: [sipx-media, sipx-rtp]
predicate:
announcement:
note: a spec sentence says the branch is unreachable · M-79 made it reachable, and it increments nothing
---

# Make the SRTP protect-error discard tell the truth

## Goal

Bring the SRTP protect-error branch back into agreement with the rule `docs/specs/media-runtime.md`
states: every discard site either increments exactly one counter, or carries a reason no longer
capable of being false.

## Why

`docs/specs/media-runtime.md` says the SRTP and SRTCP protect-error branches need no counters
because "those branches receive bytes from `Packet::encode` and `Rtcp::encode_compound`, which
always make complete headers", and `crates/sipx-media/src/session.rs` repeats it at the site:
"structurally unreachable here".

That was true while every packet the send loop protected was built internally with no extension.
`M-79` made `Encoded.extension` public, so a caller can hand `send_encoded` an extension whose
embedded length word claims more 32-bit words than are present. `Packet::encode` forwards it
verbatim and sets the X bit, `rtp_header_len` then computes a header longer than the packet and
returns `None`, and `protect` returns `TooShort`. The branch is reached, one `warn!` is emitted, the
packet is dropped, and **no counter moves** — silent media loss on an encrypted leg, at the one site
the spec promised could not happen.

Bounded, and not a security defect: it is caller-induced rather than attacker-induced. The relay
path is safe, because `Packet::decode` bounds-checks a received extension. Nothing is weakened about
what SRTP authenticates or encrypts.

## Acceptance

- [x] The site and `docs/specs/media-runtime.md` agree again. Exactly one of: a discard counter for
      the protect-error branch, validation of a caller-supplied extension at `send_encoded`'s
      boundary, or a restated reason that is true of the code as it now stands.
      → a counter: `MediaDiscardCounts::srtp_protect_failures`
      (`crates/sipx-media/src/counters.rs:32`), incremented at
      `crates/sipx-media/src/session.rs:3265`; `docs/specs/media-runtime.md` §4 rewritten to
      require it.
- [x] A failing-first test hands `send_encoded` an extension whose length word overstates its bytes
      on an SRTP leg, and asserts whatever the chosen answer promises — a counter that moves, a
      typed refusal at the boundary, or the documented drop.
      → `crates/sipx-media/tests/srtp.rs`
      `a_packet_srtp_cannot_protect_is_counted_rather_than_lost_silently`, over all three profiles.
- [x] Whatever is chosen holds for **SRTCP's** sibling branch too, or the difference is stated.
      → the difference is stated, in the spec and at
      `crates/sipx-media/src/session.rs:4265`: no public API hands bytes to
      `Rtcp::encode_compound`, so that branch stays unreachable and a counter there would be stuck
      at zero.
- [x] `Encoded.extension`'s rustdoc stops promising that "whatever header extension the `Encoded`
      carries goes out on the same packet": an extension shorter than four bytes is already silently
      dropped by `Packet::encode`'s length filter, which the doc does not mention either.
      → `crates/sipx-media/src/session.rs:745-763` and the corrected sentence on `send_encoded`,
      `crates/sipx-media/src/session.rs:2413-2425`.
- [x] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from the independent review of `M-79`'s diff, with the exact reachability chain
  traced through `Packet::encode`, `rtp_header_len` and `SrtpContext::protect`.
- 2026-08-08: **implemented, with a counter** — `MediaDiscardCounts::srtp_protect_failures`.

  *Why a counter and not the other two.* A restated reason cannot be made true: §4's rule allows a
  reason only where **no** counter can truthfully reach the site, and this site is now reachable
  from public API, so a counter is the only honest thing that fits the rule as written. Validation
  at `send_encoded`'s boundary would need a typed refusal — the method returns `bool`, where
  `false` already means "this session has stopped", and overloading it would be a second silent
  ambiguity rather than a fix. Changing the return type reaches `sipx-app`'s realtime sink trait
  and its tests, well outside this story, and it would still leave the in-bounds case (see `M-84`)
  untouched, so it buys less than it costs here. The drop itself was already right: sending in the
  clear defeats the encryption the far end negotiated.

  *SRTCP is the stated difference, not the same answer.* `Rtcp::encode_compound` builds every
  octet of a report from session state — SSRC, CNAME, counters — and always emits at least the
  eight octets `protect_rtcp` requires. No public API hands it bytes the way `Encoded::extension`
  hands them to `Packet::encode`, so that branch is still structurally unreachable and a counter
  there would be published stuck at zero. The site says so, and names the asymmetry rather than
  repeating the old sentence.

  *Failing-first (`crates/sipx-media/tests/srtp.rs`,
  `a_packet_srtp_cannot_protect_is_counted_rather_than_lost_silently`).* At the merge base
  `18a1c4d`, with the counter assertion written against `discard_counts().total()` because the
  named field did not exist yet:

  ```
  thread 'a_packet_srtp_cannot_protect_is_counted_rather_than_lost_silently' panicked at
  crates/sipx-media/tests/srtp.rs:313:13:
  AeadAes256Gcm: SRTP refused to protect the packet and it was dropped, and no discard counter
  moved — the loss is invisible to an operator
  test result: FAILED. 0 passed; 1 failed
  ```

  After: `test result: ok. 1 passed`, with `srtp_protect_failures == 1`, `total() == 1`, and
  nothing on the far end's socket — the counter names the site precisely, since no other counter
  can reach that path.

- 2026-08-08: **`M-84` filed** from this work. The in-bounds version of the same malformed
  extension — a length word that overstates its bytes without running past the packet — is refused
  by nothing, and makes SRTP treat 32 octets of media as header, so they go out unencrypted with
  no drop and no counter. Verified under all three profiles before filing. Caller-induced only: a
  relayed extension is self-consistent because `Packet::decode` slices exactly `4 + words * 4`.

- 2026-08-08: gate not run (one gate per wave, by instruction). Verified in this worktree:
  `cargo test -p sipx-media -p sipx-rtp --all-features`, `cargo clippy … -D warnings`,
  `cargo fmt --all --check`, `cargo doc … --no-deps --all-features`, `check-fixed-sleep.py`,
  `check-audio-claims.py`, `check-provenance.sh`, and both discard-site scans. All green.

  CHANGELOG sentence owed, for the coordinator to place:

  > `MediaDiscardCounts::srtp_protect_failures` counts RTP packets dropped because SRTP refused to
  > protect them. The branch was documented as unreachable until `M-79` made `Encoded::extension`
  > public: an extension whose length word overstates its bytes makes a header longer than the
  > packet, and the resulting drop moved no counter.

- 2026-08-08: **the fixture this story was proved with is now unreachable, and the pointer above is
  stale on purpose rather than by neglect.** `M-85` refused a self-inconsistent extension at
  `Packet::encode`, which is strictly earlier than the branch this story counted: the 255-words-over
  -eight-octets fixture is the same disagreement further out, so it never reaches
  `SrtpContext::protect` any more. `M-85` retargeted the test to what now happens and renamed it
  `an_extension_overstating_past_the_packet_costs_no_packet` — media arrives, the extension does
  not, and `srtp_protect_failures` stays at zero. The old name in this file's Acceptance and
  Progress is what the row was satisfied by on the day, and is left readable as history.
  **`srtp_protect_failures` is kept.** It is published, and `protect` retains cipher-level failure
  paths no argument here closes — but nothing a *caller* does can reach it now, which is one step
  from the "a field stuck at zero" that `docs/specs/media-runtime.md` §4 argues against twice.
  `M-90` is filed to settle that before the freeze.

- 2026-08-08: closed at the `1.0.0-rc.11` boundary, against the wave gate run on this tree.
