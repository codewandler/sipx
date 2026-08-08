---
id: M-90
title: Decide, before the freeze, whether a counter nothing can reach stays published
pillar: Media
status: done
priority: 7
design:
epic: media
areas: [sipx-media]
predicate:
announcement:
note: M-85 closed the only route to srtp_protect_failures · a published field that can no longer rise is the thing §4 argues against
---

# Decide, before the freeze, whether a counter nothing can reach stays published

## Goal

Settle what happens to `MediaDiscardCounts::srtp_protect_failures` now that no caller can make it
rise, while the answer is still cheap. `1.0.0` freezes the snapshot's shape; after that, a field
published stuck at zero is permanent.

## Why

`M-81` added the field because `M-79` had made the branch reachable: an extension whose length word
overstated its bytes produced a header longer than the packet, `SrtpContext::protect` refused it,
and the packet was dropped with nothing counting it. `M-85` then refused that extension one boundary
earlier, at `Packet::encode`, for both the out-of-bounds and the in-bounds case — so nothing
`Packet::encode` produces has a header the transform cannot read, and the only reachable route to
the counter is closed.

`docs/specs/media-runtime.md` §4 argues twice against exactly this shape: the ICE output naming no
bound socket "carries that reason instead of a field permanently stuck at zero", and so does the
SRTCP protect-error branch. `srtp_protect_failures` is now in that position, with two differences
that are why this is a decision rather than a deletion. It is **published** — it shipped, and
removing it is a breaking change to a snapshot type. And `protect` still has cipher-level failure
paths (`aead_seal`, `keystream`) that no argument in the spec closes, so "unreachable" is a claim
about the packet layer rather than about the function.

`M-81`'s own lesson applies to its own field: *"unreachable because of what the caller can be" is a
claim with an expiry date*. This story is that claim being re-examined once, deliberately, rather
than discovered again by whoever next makes the branch reachable.

## Acceptance

- [x] One of three, argued in `docs/specs/media-runtime.md` §4: the field stays with its site and a
      restated reason; the field stays and the site carries a `// discard:` reason instead; or the
      field is removed with a `CHANGELOG.md` migration note saying what to read instead.
      → **the third**: the field is removed (`crates/sipx-media/src/counters.rs`), the site carries a
      `// discard:` reason (`crates/sipx-media/src/session.rs:3304`), and the argument is
      `docs/specs/media-runtime.md` §4. The `CHANGELOG.md` sentence is drafted verbatim under
      `## Progress` below — that file is the coordinator's to write and was not touched here.
- [x] Whichever is chosen, `docs/specs/media-runtime.md` §4's paragraph on the branch says what an
      operator should conclude from the number they see, including zero.
      → `docs/specs/media-runtime.md` §4, "What an operator reads instead is
      `malformed_extensions_dropped`": what zero there means, that there is no second number for the
      same mistake, and that the `dropping a packet SRTP could not protect` log line is a defect in
      this stack rather than an event on the call.
- [x] If it stays, a test pins that the branch is still wired — a unit-level `protect` refusal, or
      an argument for why no test can reach it that does not repeat the reason `M-79` invalidated.
      → **n/a, the field went** — and the row's substance was answered anyway, because the
      removal's premise needs the same guard. `crates/sipx-rtp/tests/srtp_protect_header.rs` holds
      both halves: `protect` still refuses a header it cannot read (two unit-level refusals), and
      nothing `Packet::encode` produces is one, over every adversarial extension and over-long CSRC
      list a caller can reach it with. That test failing is the notice that the branch owes a
      counter again.
- [x] `./scripts/gate.py` green.

## Notes

- Sites: `crates/sipx-media/src/counters.rs` (the field),
  `crates/sipx-media/src/session.rs` (the protect-error branch),
  `docs/specs/media-runtime.md` §4 (the argument this has to join).
- Related: `M-80` is the other freeze-deadline decision on this crate's published shape.

## Progress

- 2026-08-08: filed from `M-85`'s implementation, which closed the route and left the field behind.
  `M-85` kept both the field and the increment deliberately — it is not this story's job to be done
  in passing — and recorded the reasoning at the site and in §4.

- 2026-08-08: **decided — the field is removed.** `protect` was enumerated as a function rather than
  as a branch first, because the story's own framing is that "unreachable" had been a claim about
  the packet layer. It has exactly four failure points and a session can reach none of them:

  | `protect` failure | Reached by | Evidence |
  |---|---|---|
  | `rtp_header_len` → `TooShort` — the header runs past the packet | no | `crates/sipx-rtp/src/packet.rs:104-137`: `encode` writes the twelve-octet fixed header, caps CSRCs at `min(15)` and takes exactly that many, and since `M-85` writes an extension only when `extension_is_self_consistent`. The computed header is then always the written one. |
  | `sequence_and_ssrc` → `TooShort` — under twelve octets | no | strictly implied by the first: `rtp_header_len` rejects `len < 12` before this is reached. |
  | `aead_seal` → `KeyLength`, then `TooShort` on `P_MAX` | no | `crates/sipx-rtp/src/srtp/mod.rs:388-421`: `Context::new` measures the master key and salt against the profile and sizes the session key at `profile.key_and_salt_len()`. `aes-gcm` 0.10.3's `P_MAX`/`A_MAX` are `1 << 36` — 64 GiB, against a datagram. |
  | `keystream` → `KeyLength` on session salt or key | no | same constructor; reached only under `AesCm128HmacSha1_80`, whose salt is 14 and key 16, which is what `keystream` requires. |

  No fifth path exists: RFC 3711 §9.2's master-key packet lifetime is **not** enforced here and
  `SrtpError` has no variant for exhaustion, so a long call cannot age its way into this branch, and
  the ROC/`highest_seq` state the send loop advances is infallible. The send loop at
  `crates/sipx-media/src/session.rs:3302` is the only non-test caller of `protect` in the workspace,
  and it protects exactly `packet.encode()`.

  So the field was in precisely the position §4 rejects twice, and the sentence that had exempted it
  — *"the difference is where the bytes come from"* — was no longer true: `Packet::encode` now builds
  a header out of numbers that agree, exactly as `Rtcp::encode_compound` does. Applying §4's own rule
  to a case its own criterion no longer distinguishes gives removal. It was weighed against keeping
  the field as insurance, since `MediaDiscardCounts` is exhaustive and re-adding a field after
  `1.0.0` costs a major release; that argument lost because it would equally keep a field for every
  branch that might one day become reachable, and it is recorded in §4 rather than only here.

  **`CHANGELOG.md` sentence owed** — coordinator-owned, drafted here verbatim for `[Unreleased]`
  under `### Removed`. The field shipped in `1.0.0-rc.11`, so this is breaking:

  > **Breaking: `MediaDiscardCounts::srtp_protect_failures` is removed.** Read
  > `malformed_extensions_dropped` instead: it counts the same caller mistake — a header extension
  > whose length word disagrees with its bytes — one boundary earlier, at `Packet::encode`, where
  > the extension is dropped and the payload still sent. `rc.11` closed the only route by which a
  > caller could make the protect failure happen, leaving a published field nothing could move;
  > `MediaSession::discard_counts` and `MediaDiscardCounts::total()` are otherwise unchanged, and no
  > other field's meaning moved. A reader that named the field now fails to compile rather than
  > reading a permanent zero.

  Verification, in the worktree: `cargo test -p sipx-media -p sipx-rtp --all-features` (22 suites
  green), `cargo check --workspace --all-features --all-targets`, `cargo clippy -p sipx-media
  -p sipx-rtp --all-targets --all-features --no-deps -- -D warnings`, `cargo doc`,
  `cargo test -p sipx-media --test discards`, `./scripts/check-audio-claims.py --check`,
  `./scripts/check-fixed-sleep.py --check`, `./scripts/check-provenance.sh`. `./scripts/gate.py` is
  the coordinator's wave run and is why that row stays unticked.

  Open for whoever closes this: `MediaDiscardCounts` has public fields and no constructor, so
  `scripts/check-audio-claims.py` does not hold it to `#[non_exhaustive]` and `M-92` carries that
  question. `crates/sipx-media/src/counters.rs`'s new exhaustive-literal test is a unit test inside
  the crate for exactly that reason — `#[non_exhaustive]` would not break it — but `M-92` should
  read it before deciding, because that decision also fixes whether a counter can ever be added
  back after `1.0.0`, which is the one cost this story accepted.

- 2026-08-08: closed at the `1.0.0-rc.12` boundary, against the wave gate run on this tree.
