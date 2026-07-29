---
id: X-30
title: Make the registry distinguish "implemented in a crate" from "reachable from a call"
pillar: Build
status: in-progress
priority: 2
design: docs/designs/rfc-registry-grain.md
epic: conformance
areas: [docs, sipx-testkit]
note: fifth instance in two days — rfc-report.py --check verifies that cited files exist, never that a claimed capability has a caller, so a crate-level feature reads as a shipped one
---

# Make the registry distinguish "implemented in a crate" from "reachable from a call"

## Goal
Stop `docs/compliance.md` reporting a capability as shipped when nothing above the crate that
implements it can reach it. The table is linked from the README as a *measurement*, and this is the
one way it has repeatedly been wrong.

## Acceptance
- [x] A mechanical check that an entry claiming a role cites evidence at or above the layer that
      makes the role reachable. The concrete proposal from `M-28`: an entry may not claim `uac` or
      `uas` unless its `evidence` cites a file at or above `sipx-call`. Validate the rule against
      the whole registry before adopting it — if it produces a wave of false positives it is the
      wrong rule, and saying so is a real outcome of this story.
      → `unreachable_role_claims` in `scripts/rfc-report.py`. **Validated, and the rule as stated
      does not hold**: it rejects 22 of 29 role-claiming rows, and only 7 of those rejections point
      at anything real. Adopted narrowed to `layer = "media"` — a choice, argued on the merits in
      `docs/designs/rfc-registry-grain.md`, which carries the row-by-row count.
- [x] The check runs in `./scripts/rfc-report.py --check`, which is already a gate step and a CI
      job, so a new over-claim fails the gate rather than being found by the next story to touch
      that code.
      → called from `check()`, so `--check`, the gate's `rfc compliance` step and the `docs` CI
      job all run it. Gate green: 18 steps.
- [x] Every row the new check rejects is corrected in the same commit — or the check is narrowed,
      with the reason recorded. Landing a check plus a suppression list is how this stops working.
      → four rows rejected, four corrected, **no suppression list**: 8122, 8445 and 8839 lost
      roles they could not support; 3711 gained the two call-layer citations that show it can.
      The narrowing to media is recorded in the design doc and in `docs/rfc/README.md`.
- [x] Failing-first test: a fixture registry entry claiming both roles while citing only a leaf
      crate passes `--check` today. Name the test that makes it fail. `scripts/test-rfc-report.py`
      is where the existing checks are tested.
      → `RoleReachability.test_a_media_role_claimed_from_a_leaf_crate_is_rejected`.

## Progress
- **Done.** The rule from `M-28` was measured before adoption and **does not hold as stated** —
  22 of 29 role-claiming rows rejected, only 7 of those rejections real (3 over-claims, 4 rows
  whose evidence was merely incomplete). `evidence` cites the code that *implements* a behaviour,
  which says nothing about whether a call reaches it; every call reaches the transaction layer,
  DNS and offer/answer.
- Adopted narrowed to the `media` layer. That is a **choice**, not something the workspace forced:
  media is the one layer where the crate serving a role (`sipx-call`) and the crate implementing
  the capability (`sipx-media`, `sipx-sdp`) come apart, which is the gap ICE and DTLS-SRTP fell
  into. Scoped that way it rejects four rows and three are genuine.
- **Corrected after review:** the first version justified the scope by claiming seven `sipx-ua`
  rows could not satisfy the rule "at any price" because `sipx-ua` is a sibling of `sipx-call`.
  That is false — `crates/sipx-cli/Cargo.toml` names both, and `sipx-cli/src/register.rs` is a
  real caller of the auth path. The scope survives on its own merits; the false justification and
  the already-fired widening trigger it produced are replaced, and the error is recorded in the
  design doc so the next author does not inherit "structurally impossible".
- RFCs 3680, 3856, 3903 and 4235 are addressed explicitly rather than counted as false positives:
  they keep their roles because `crates/sipx-ua/tests/packages.rs` imports `sipx_ua::presence` and
  `sipx_ua::packages` across the crate boundary, which is the external caller ICE and DTLS-SRTP
  have nowhere. `sipx-cli` not using them is a CLI gap, not a false claim.
- Corrected: **8122** (DTLS fingerprint, `implemented`+both roles → `partial`, no roles — the
  `a=fingerprint` branch in `sipx-call` is dead because `Capabilities::with_dtls_srtp` has no
  caller outside `sipx-sdp`'s tests); **8445** and **8839** (ICE, both roles → none —
  `MediaSession::gather` and `start_with_ice` have no caller outside `sipx-media`; `M-27` is the
  wiring story); **3711** (SRTP *is* reachable — evidence now cites
  `crates/sipx-call/tests/secure_media.rs` and `crates/sipx-cli/tests/interop_srtp.rs`).
- The reachable set is derived from the workspace manifests (`call_layer_crates`), not listed.
  Only `crates/…` paths count: the repo-root `tests/` escape hatch was removed, since `evidence`
  may cite markdown and `tests/interop/README.md` would otherwise have proved reachability.
- RFC 3711's note said `secure_media.rs` carries audio "in each direction". It plays one
  direction; both ends assert `is_encrypted()`, and the bidirectional evidence is `interop_srtp.rs`,
  which is `#[ignore]`d and runs only in the interop matrix. The note now says exactly that.
- Deliberately not done: a cross-crate caller check, which would bind to reachability itself
  rather than to evidence paths. The current check can be satisfied by citing a call-layer file
  containing a dead branch — that is 8122's exact shape. Recorded under "what would widen this",
  alongside the fact that `layer` is author-chosen, so relabelling a media row exits the check.
- Loose end, not taken: RFCs 2617, 7616 and 8760 could cite `crates/sipx-cli/src/register.rs` and
  `crates/sipx-cli/tests/cli.rs` and would then satisfy even the unscoped rule. The media-scoped
  check does not ask them to, so the citations were left alone rather than churned.

## Notes
- **Five instances in two days, which is the argument.** `M-22` built ICE no call could offer
  (`M-27`); RFC 3311 claimed an UPDATE role only the answering end could reach (`S-22`); `M-15`
  built DTLS-SRTP no call can select, marked `implemented` for both roles (`M-28`); `M-26` built
  RFC 4568's §5.1.3 check that no call ran (`M-29`); and RFC 8122 still carries the same shape
  today, untouched because `M-28`'s Acceptance named only 5763 and 5764.
- **The blind spot is structural, and `rfc-report.py` cannot see it by design.** It verifies that a
  named header is known to the parser, that a cited file exists, and that a claim cites *something*.
  All three pass for a capability whose only caller is its own test module. As `M-28` put it:
  unreachable code is untested code with better paperwork.
- Anticipated by `M-28`'s own Notes ("worth asking, once, whether the registry should distinguish
  implemented-in-a-crate from reachable-from-a-call — that is a candidate `X` story of its own")
  and recommended again by its implementor at handoff. Filed at integration.
- **Check `docs/designs/rfc-registry-grain.md` first.** Requirement-level grain was considered and
  declined there, and that design also records what would reopen the question — this story must
  either fit the existing grain or make the case for changing it, not quietly add a third axis.
- The `roles` key is the natural place to hang this because it is already the thing that over-claims:
  `M-28` fixed two rows by *removing* `roles` rather than by adding a caveat, and `S-22`'s fix was
  to name which handle serves which role. Both suggest the rule belongs on roles rather than on
  `status`.
