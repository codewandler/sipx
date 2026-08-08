---
id: M-85
title: Refuse a header extension that disagrees with itself, at the send boundary
pillar: Media
status: in-progress
priority: 5
design:
epic: media
areas: [sipx-media, sipx-rtp]
predicate:
announcement:
note: M-81 counted the packets SRTP refuses · the in-bounds case is refused by nothing, and sends media octets outside the encryption
---

# Refuse a header extension that disagrees with itself, at the send boundary

## Goal

Stop the send path from putting a caller-supplied RTP header extension on the wire when its
embedded length word disagrees with the bytes that follow it. `M-81` made the case that fails
loudly visible; this is the case that fails quietly, and the consequence is worse than a lost
packet.

## Why

`M-79` made `Encoded::extension` public and nothing validates it. `M-81` handled the extension
whose length word overstates its bytes *far enough to run past the whole packet*: `rtp_header_len`
returns `None`, `SrtpContext::protect` refuses, and the packet is now dropped and counted as
`srtp_protect_failures`.

The **in-bounds** version of the same mistake is not refused by anything. An extension that claims
nine 32-bit words and carries one still leaves the computed header inside the packet, so
`rtp_header_len` accepts it and the header/payload split lands 32 octets *inside the media*. Those
octets are then treated as header: authenticated but not encrypted under counter mode, Associated
Data under the AEAD profiles (RFC 7714 §8.2). They go out in the clear on a leg the far end
negotiated encryption for, and nothing is dropped, counted or logged, because from the transform's
point of view nothing went wrong.

Measured, not reasoned: an 8-octet extension declaring nine words, on a 160-octet µ-law payload,
protected under each of the three profiles, put a 32-octet run of plaintext µ-law on the wire —
`AeadAes256Gcm: len=196 plaintext_runs=17` for windows of 16 identical octets. The far end also
reads the payload boundary from the same length word, so it decodes 32 octets short.

**Bounded, and caller-induced rather than attacker-induced.** A relayed extension cannot do this:
`Packet::decode` slices exactly `4 + words * 4` octets and rejects a length word that runs past the
datagram, so an extension that arrived over the network is self-consistent by construction
(`crates/sipx-rtp/src/packet.rs:141`). Reaching this needs the application on *this* side to build
`Encoded::extension` by hand and get the length word wrong. Nothing about what SRTP authenticates
or encrypts is weakened — the transform does exactly what it was asked to; it is asked the wrong
question.

The plain-RTP leg has the same shape without the confidentiality edge: the packet goes out as
written and the far end rejects it, silently, with no counter on either side.

## Acceptance

- [x] A failing-first test protects a packet whose caller-supplied extension declares more words
      than it carries but stays inside the packet, and asserts no plaintext payload octet reaches
      the wire.
      → `crates/sipx-media/tests/srtp.rs`
      `a_self_inconsistent_extension_puts_no_plaintext_media_on_the_wire`, over all three profiles,
      asserting on the datagram's octets rather than on a counter.
- [x] One boundary refuses it, and the choice is argued in `docs/specs/media-runtime.md`: either
      `Packet::encode` writes no extension it cannot make self-consistent — extending the length
      filter it already applies below four octets — or `send_encoded` refuses the frame with a
      typed error. If the extension is dropped rather than the packet, say what a caller is
      promised, because the extension is metadata the far end may be relying on.
      → `Packet::encode` (`crates/sipx-rtp/src/packet.rs:89`), over the published predicate
      `sipx_rtp::extension_is_self_consistent` (`crates/sipx-rtp/src/packet.rs:227`); argued in
      `docs/specs/media-runtime.md` §4.4, including what a caller is promised when the extension is
      the thing dropped.
- [x] Whatever is chosen, an operator can see it happened: a counter, or a documented reason no
      counter can reach it under `docs/specs/media-runtime.md` §4's rule.
      → `MediaDiscardCounts::malformed_extensions_dropped` (`crates/sipx-media/src/counters.rs:46`),
      incremented at `crates/sipx-media/src/session.rs:3242`.
- [x] The plain-RTP leg is covered by the same answer, or the difference is stated.
      → same refusal, same counter, and the difference in what the mistake costs is stated in §4.4;
      `crates/sipx-media/tests/relay_extension.rs`
      `a_self_inconsistent_extension_costs_a_plain_leg_no_media`.
- [x] `Encoded::extension`'s rustdoc is updated to describe what actually happens after this
      change; `M-81` documented the behaviour this story replaces.
      → `crates/sipx-media/src/session.rs:745-771`, and the corrected sentence on `send_encoded`
      at `crates/sipx-media/src/session.rs:2419-2424`.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: **implemented — the refusal is at `Packet::encode`, and the extension is dropped
  rather than the packet.**

  *Why that boundary.* The predicate is a property of the extension alone, so it belongs beside the
  bytes rather than at a session: `sipx_rtp::extension_is_self_consistent` says an extension is
  `4 + words * 4` octets and nothing else, `Packet::encode` filters on it, and the four-octet
  filter that used to stand there is now the total case of the same rule. That reaches every caller
  of the packet layer and **both legs** in one place. A typed error at `send_encoded` would change
  a published signature and reach the application layer's realtime sink trait — the reason `M-81`
  did not take that route — and would still leave a plain leg writing the bad bytes, which is where
  the mistake costs media rather than secrecy. Rewriting the length word was rejected as a guess:
  a wrong word says nothing about which of the two numbers the caller meant.

  *What a caller is promised*, since the extension is what gets dropped: the payload goes out
  whole, the extension does not, and the drop increments
  `MediaDiscardCounts::malformed_extensions_dropped`. The far end sees a packet with no extension,
  which is what it sees whenever this side has no metadata to attach, so a receiver relying on an
  RFC 8285 element is handed none rather than a wrong one. `docs/specs/media-runtime.md` §4.4
  carries the argument and D16–D18 the vectors.

  *Failing-first, on the bytes.* `crates/sipx-media/tests/srtp.rs`
  `a_self_inconsistent_extension_puts_no_plaintext_media_on_the_wire`, at the merge base
  `1c7d1c4`:

  ```
  thread 'a_self_inconsistent_extension_puts_no_plaintext_media_on_the_wire' panicked at
  crates/sipx-media/tests/srtp.rs:411:9:
  assertion `left == right` failed: AeadAes256Gcm: 17 window(s) of 16 identical payload octets
  left the socket in the clear on an encrypted leg — the extension's length word moved the header
  boundary into the media (len=196)
    left: 17
   right: 0
  test result: FAILED. 0 passed; 1 failed
  ```

  That reproduces the filed measurement exactly — `len=196`, 17 windows — and it is the leak
  itself, not a counter standing in for it. After: `ok`, with `malformed_extensions_dropped == 1`,
  `total() == 1`, and the far end decrypting the full 160-octet payload with no extension on it.

  The plain leg, `crates/sipx-media/tests/relay_extension.rs`
  `a_self_inconsistent_extension_costs_a_plain_leg_no_media`, at the same base:

  ```
  assertion `left == right` failed: the far end read 32 octets of media as header, because the
  length word this side wrote says the payload starts later than it does
    left: 128
   right: 160
  ```

  And at the packet layer, `crates/sipx-rtp/tests/header_extension.rs`
  `encode_refuses_an_extension_that_disagrees_with_its_own_length_word`: `left: 16, right: 0` on
  the X bit, over declared lengths of nine, zero and two words.

- 2026-08-08: **one correction to this story's own text, measured.** On a plain leg the far end does
  not reject the packet. The computed header stays inside it, so the packet is well formed by every
  check a receiver can apply: it accepts it and delivers 128 octets of a 160-octet payload, with
  nothing to log, because only the sender ever knew where the boundary was meant to be. Recorded in
  §4.4 rather than left as an inherited claim.

- 2026-08-08: **`M-81`'s test was retargeted, and its counter is now unreachable from public API.**
  `Packet::encode` refuses `M-81`'s fixture too — 255 words over eight octets is the same
  disagreement, further out — so that extension no longer reaches `SrtpContext::protect` and
  `srtp_protect_failures` no longer rises from anything a caller can do. The test that proved it
  did is now
  `crates/sipx-media/tests/srtp.rs::an_extension_overstating_past_the_packet_costs_no_packet`,
  asserting the new outcome over `M-81`'s own fixture: the media arrives, the extension does not,
  `malformed_extensions_dropped == 1` and `srtp_protect_failures == 0`. **`M-81`'s Acceptance cites
  the old test name** (`a_packet_srtp_cannot_protect_is_counted_rather_than_lost_silently`) and
  needs that pointer updated; that file is outside this story's fence.

  The counter and its increment site stay: the field is published, `protect` has cipher-level
  failure paths no argument closes, and a non-zero value is now a precise statement — something
  inside the crate handed the transform a packet it could not read. `M-90` asks whether it should
  still be in the published snapshot at the freeze.

- 2026-08-08: gate not run (one gate per wave, by instruction). Verified in this worktree:
  `cargo test -p sipx-media -p sipx-rtp --all-features`, `cargo clippy … -D warnings`,
  `cargo fmt --all`, `cargo doc … --no-deps --all-features`, `check-fixed-sleep.py --check`,
  `check-audio-claims.py --check`, `check-provenance.sh`, `check-docs-links.py`,
  `check-app-surface.py --check`, and `cargo test -p sipx-media --test discards`. All green.

  CHANGELOG sentence owed, for the coordinator to place:

  > An RTP header extension whose embedded length word disagrees with the bytes behind it is no
  > longer written onto a packet. `Packet::encode` leaves it off, the payload is sent whole, and the
  > drop is counted as `MediaDiscardCounts::malformed_extensions_dropped`; the new predicate
  > `sipx_rtp::extension_is_self_consistent` is the question it asks. Such an extension used to be
  > written verbatim, which put a run of unencrypted media on the wire on an SRTP leg — the header
  > the transform computed reached into the payload — and truncated the media a plain leg's peer
  > played. Only a caller-built `Encoded::extension` could reach it; a relayed extension is bounds
  > checked when its packet is decoded.

- 2026-08-08: filed from `M-81`'s implementation, which counted the out-of-bounds case and verified
  this one empirically while checking the reachability chain. A typed refusal at `send_encoded`
  changes a published signature and reaches `sipx-app`'s realtime sink trait, which is why `M-81`
  did not take that route for the narrower bug — worth deciding once, here, for both.

## Notes

- Sites: `crates/sipx-rtp/src/packet.rs:76` (`encode`'s existing four-octet filter),
  `crates/sipx-rtp/src/srtp/mod.rs:1079` (`rtp_header_len`),
  `crates/sipx-media/src/session.rs:2425` (`send_encoded`).
- Priority above `M-81`'s because the failure is silent and touches confidentiality on an encrypted
  leg, and below anything attacker-reachable because it is not.

- 2026-08-08: renumbered from `M-84` on filing; that id was allocated in the same wave to carrying
  calibrated thresholds onto the application wire. Only the id moved. This remains the more serious
  half of what `M-81` uncovered: an extension whose length word overstates its bytes while keeping
  the computed header inside the packet is refused by nothing, and puts a plaintext run of media on
  the wire under all three protection profiles.
