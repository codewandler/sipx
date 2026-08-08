---
id: C-8
title: Relay the early negotiation carriers
pillar: Call
status: in-progress
priority: 35
design: docs/designs/edge.md
epic: edge
areas: [sipx-call, sipx-sdp]
predicate:
announcement:
note: C-7 refuses reliable provisionals, PRACK and offerless INVITE rather than half-relaying them · relaying those means authoring a description this role has no media for
---

# Relay the early negotiation carriers

## Goal

Extend the off-media coupling role to the negotiation carriers `C-7` deliberately refused, without
the coupling ever authoring a media description of its own.

## Acceptance

- [x] A reliable provisional response carrying a description is relayed across both dialogs, with
      `100rel` no longer stripped from the target INVITE, and PRACK correlated on both legs.
- [x] An offerless INVITE is relayed as a delayed offer rather than refused with 488, with the
      answer mapped back on the source leg. *Partial in one direction, deliberately: RFC 3264's
      other carrier — offer in the 2xx, answer in the ACK — is still refused, because a source
      that does not offer `100rel` leaves no reliable provisional to relay through and this stack
      implements that carrier on neither side. Filed as `C-9`.*
- [x] The `C-7` invariants hold unchanged on every new carrier: the coupling holds no
      `MediaSession`, binds no RTP, advertises no sipx address, and refuses an unmappable
      description on its source leg before the peer leg is told.
- [x] A live causal test proves media flows endpoint-to-endpoint across each newly supported
      carrier, as `C-7`'s does for `InitialInvite`, `Update` and `Reinvite`.
- [x] `docs/rfc/registry.toml`'s RFC 7092 evidence is extended in the same commit, and the status is
      raised only for what is actually proven — `C-7` deliberately left it `partial`.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `C-7`, which named this gap and could not file it because the board is
  fenced from implementors. `C-7` refuses rather than half-delivers: the target INVITE omits
  `100rel`, so RFC 3262 §3 forbids the peer sending a reliable provisional at all, and an offerless
  INVITE gets 488. Both carriers would require this role to author a description — that is, to
  describe a media endpoint it does not have — which is the one thing the role exists not to do.

- 2026-08-08: implemented. The premise `C-7` refused on turned out to be wrong in an interesting
  way: neither carrier needs this role to author anything, because in both of them *some endpoint*
  writes the description and the coupling only has to carry it to the other one.

  - **`100rel` is mirrored, not asserted.** `offer_invite` copies the source INVITE's `Supported`
    and `Require` onto the target INVITE. That is the whole reason the carrier is safe: RFC 3262 §3
    lets the target put a description in a reliable provisional only if this request says the
    extension is supported, and a description arriving there can be carried onward only if the
    source leg accepts one too. Asserting `100rel` unconditionally would leave the role holding a
    description with nowhere to put it. `Allow` gains PRACK for the same reason.
  - **`EarlyCarriers`** is the new state, and it is bookkeeping rather than a second machine: the
    numbering of the provisionals this side sends on the source leg (§3), the ordering of the ones
    it receives on the target leg (§4), the target early dialog, and the PRACK held for an answer.
    Offer/answer policy is still the one `CouplingState` — `complete(Leg::One)` for an answer that
    arrived in a provisional, `begin_offer(Leg::Two, ReliableProvisional)` for a delayed one.
  - **PRACK is correlated on both legs.** On the source leg the match is §3's — `RAck` against the
    number this side allocated, the source INVITE's own `CSeq` and method — and a PRACK that
    matches nothing gets `481`. On the target leg this side is the UAC. The target PRACK carries a
    body in exactly one case, §5's: the INVITE offered nothing, so the answer to the delayed offer
    is in the source's PRACK and nowhere else. It is held until that answer arrives.
  - **`prack_target` does not re-serialize.** `crate::rel::send_prack` takes a parsed
    `SessionDescription`, which would normalize line order, TTLs and `m=` port counts — the exact
    liberty `C-7` established this role may not take. The local builder carries the bytes.
  - **The confirmed dialogs inherit the early numbering.** A PRACK sent while the target dialog was
    early consumes a number in this side's sequence space, and a dialog rebuilt from the 2xx would
    hand it out twice (RFC 3261 §12.2.1.1). Same on the source leg for the peer's numbering.
  - **The 2xx carries a description only when one is still owed.** An exchange §5 settled in the
    provisional is complete; repeating it would be a renegotiation nobody asked for. A 2xx with no
    description and no settled exchange is a target that never answered, and fails.
  - `crate::rel::retransmit_until_pracked` became `pub(crate)` and takes a `CancellationToken`
    instead of an `Arc<Notify>`. Not cosmetic: a notification with no waiter is lost, so an
    acknowledgement arriving before the task first polls would leave it resending for the full 32
    seconds. `Ringing` gets the fix for free.

  Failing-first, at `3ba82b9` with `cargo test -p sipx-call --all-features --test off_media_coupling`:

  ```
  test a_reliable_provisional_answer_crosses_both_legs_and_early_media_follows_it ... FAILED
  test an_offerless_invite_is_relayed_as_a_delayed_offer_and_early_media_follows_it ... FAILED
  test an_unmappable_provisional_never_reaches_the_peer_leg ... FAILED

  ---- a_reliable_provisional_answer_crosses_both_legs_and_early_media_follows_it stdout ----
  panicked at crates/sipx-call/tests/off_media_coupling.rs:1214:5:
  `100rel` is no longer stripped from the target INVITE (RFC 3262 §3)

  ---- an_offerless_invite_is_relayed_as_a_delayed_offer_and_early_media_follows_it stdout ----
  panicked at crates/sipx-call/tests/off_media_coupling.rs:1407:10:
  the off-media coupling relays the delayed offer: Sdp("an off-media coupling relays
  descriptions and has none of its own to offer")

  ---- an_unmappable_provisional_never_reaches_the_peer_leg stdout ----
  panicked at crates/sipx-call/tests/off_media_coupling.rs:1682:6:
  the source INVITE receives a final response: Elapsed(())

  test result: FAILED. 6 passed; 3 failed
  ```

  All nine pass now, the six `C-7` vectors unchanged. The two media proofs are causal in the same
  shape `C-7`'s are and stronger in one respect: the audio arrives on a socket the test bound
  **before either leg has been answered**, which is only possible because the description crossed
  in the reliable provisional. `C-7` could not reach that state at all — with `100rel` stripped,
  `ring_early` refuses `421`.

- 2026-08-08: the RFC 7092 row stays `partial`, and not because §3.1.3 is incomplete. The evidence
  and note are extended, but three of the taxonomy's roles — the proxy-B2BUA §3.1.1 and the media
  relay and media-aware roles §3.2.1 and §3.2.2 — are ones sipx deliberately does not hold, and no
  amount of work on §3.1.3 changes that. Raising the row would claim the taxonomy is covered. The
  note now names the one remaining §3.1.3 carrier gap (`C-9`) separately from those three, so a
  reader can tell a deliberate absence from an unfinished one.

- 2026-08-08: `./scripts/gate.py` deliberately not run here — the wave gate covers it. Verified with
  `cargo test -p sipx-call --all-features` (every target green), `cargo clippy -p sipx-call
  --all-targets --all-features --no-deps -- -D warnings`, `cargo fmt --all`, `cargo doc -p sipx-call
  --no-deps --all-features`, `./scripts/rfc-report.py --check`, `./scripts/check-fixed-sleep.py
  --check` and `./scripts/check-app-surface.py --check`.

  CHANGELOG sentence for the coordinator: *The off-media coupling relays the early negotiation
  carriers: `100rel` is mirrored onto the target INVITE, a reliable provisional carrying a
  description crosses both dialogs with PRACK correlated on each, and an offerless INVITE is
  relayed as RFC 3262 §5's delayed offer rather than refused 488.*

## Notes

- `Invitation::answer_signalling` cannot carry a body today; a role reusing `SignallingCall` for
  this would need `prepare` to take one.
- `signalling::response_matches_dialog` hard-codes CSeq method `Bye`, so it is not reusable for
  UPDATE or re-INVITE responses. `C-7` built its own rather than widening it; this story may want
  to widen it properly. **Not touched.** Nothing this story added reaches it — the early carriers
  read their responses from the INVITE transaction rather than by dialog matching — so widening it
  would have been an unrelated change to a shared helper with its own callers.
- Left open and not this story's: an *unreliable* provisional on the target leg still reaches the
  source leg as nothing at all, so a plain `180 Ringing` from the target does not make the source
  endpoint ring. `C-7` behaved the same way and no acceptance row here names it. It carries no
  description, so it is a progress-relay gap rather than a negotiation one.
