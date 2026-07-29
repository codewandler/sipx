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
      → `unreachable_role_claims`, `scripts/rfc-report.py:151`. **Validated, and the rule as stated
      does not hold**: it rejects **22 of 29** role-claiming rows and only 7 of those rejections
      point at anything real — re-measured independently against `57857c6`, which is now the commit
      the count names, because against this story's own result the same rule gives 18 of 26 and a
      reader checking the figure would conclude the design was wrong. Adopted narrowed to
      `layer = "media"`, a **choice** argued on the merits in `docs/designs/rfc-registry-grain.md`
      ("The property the check is about is *selection*"), which carries the row-by-row count.
- [x] The check runs in `./scripts/rfc-report.py --check`, which is already a gate step and a CI
      job, so a new over-claim fails the gate rather than being found by the next story to touch
      that code.
      → called from `check()` at `scripts/rfc-report.py:262`, so `--check`, the gate's
      `rfc compliance` step and the `docs` CI job all run it. Full gate green: 18 steps.
- [x] Every row the new check rejects is corrected in the same commit — or the check is narrowed,
      with the reason recorded. Landing a check plus a suppression list is how this stops working.
      → four rows rejected, four corrected, **no suppression list**: 8122, 8445 and 8839 lost
      roles they could not support; 3711 gained the two call-layer citations that show it can.
      The narrowing to media is recorded in the design doc and in `docs/rfc/README.md`, as a choice
      with its cost, not as a constraint.
- [x] Failing-first test: a fixture registry entry claiming both roles while citing only a leaf
      crate passes `--check` today. Name the test that makes it fail. `scripts/test-rfc-report.py`
      is where the existing checks are tested.
      → `RoleReachability.test_a_media_role_claimed_from_a_leaf_crate_is_rejected`,
      `scripts/test-rfc-report.py:194`. Re-proved against `57857c6` with this branch's test file:
      *"AssertionError: False is not true : a media role no call can select was accepted;
      problems=['README.md says 70 RFCs; the registry has 1', …]"* — no `reach` problem is
      produced, because the check does not exist there.

## Progress
- **Done.** The rule from `M-28` was measured before adoption and **does not hold as stated** —
  22 of 29 role-claiming rows rejected (at `57857c6`), only 7 of those rejections pointing at
  anything true of the row (3 over-claims, 4 rows whose evidence was merely incomplete), which is
  the same thing as saying **19 of the 22 rejected rows were making a correct claim**. Both counts
  are now stated in the design, because they answer different questions and rounding one into the
  other is how a measurement stops being one. `evidence` cites the code that *implements* a
  behaviour, which says nothing about whether a call reaches it; every call reaches the transaction
  layer, DNS and offer/answer.
- Adopted narrowed to the `media` layer. That is a **choice**, argued from a property of the code:
  a media capability must be *selected* (`with_srtp`, `with_dtls_srtp`, `start_with_ice`) and
  selecting nothing is the silent default, which is exactly how ICE and DTLS-SRTP shipped
  unreachable; nothing is selected in the other layers, so "can a call reach the transaction layer"
  cannot come out `no`. Scoped that way it rejects four rows and three are genuine over-claims.
- **Corrected after review (first pass):** the original version justified the scope by claiming
  seven `sipx-ua` rows could not satisfy the rule "at any price" because `sipx-ua` is a sibling of
  `sipx-call`. That is false — `crates/sipx-cli/Cargo.toml:21-22` names both, and
  `crates/sipx-cli/src/register.rs:95` is a real caller of the auth path, exercised by
  `crates/sipx-cli/tests/cli.rs:116`. Verified independently here. It also inverted this design's
  own widening trigger, which filed "*if* an application crate came to sit above both" as a future
  condition: `sipx-cli` is that crate and always was.
- **Corrected after review (second pass, this rework):** the replacement justification was *also*
  false. It said the four `sipx-ua` service rows differ from ICE because `packages.rs` proves a
  caller across the crate boundary, "which is precisely what `MediaSession::start_with_ice` has no
  example of, in any crate, including their own integration tests".
  `crates/sipx-media/tests/ice.rs:149-150` calls `start_with_ice` twice, from an integration test
  linking `sipx_media` from outside exactly as `packages.rs` links `sipx_ua`. Had that criterion
  been the rule, 8445 and 8839 would have passed it and this story's central correction would be
  wrong. Both errors are now recorded together in the design under "Two false justifications", with
  the shared shape named: reaching for a mechanically appealing fact that is not true, when the real
  reason is a judgement about which crate serves the role.
- **The scope's real argument is selection, and it is now tested rather than asserted.** A media
  capability is carried only because something asked for it (`with_srtp`, `with_dtls_srtp`,
  `start_with_ice`); asking for nothing is the default and it is silent — the call still connects
  and every test in the crate below still passes. Nothing is selected in the other layers: there is
  no `with_transactions`, no `with_dns`, so "can a call reach the transaction layer" cannot come out
  `no`. `layer = "media"` is a **proxy** for that property, used because the check reads evidence
  paths and nothing else; checking selection means resolving callers across crates, which is a
  different check on a different input and is filed as this one's successor.
  `test_the_scope_tracks_selection_not_the_layer_string` holds proxy and property in agreement:
  `.with_srtp(` has callers in `crates/sipx-call/src/`, while `sipx-call` contains no `ice` as a
  word at all and never names `with_dtls_srtp`.
- RFCs 3680, 3856, 3903 and 4235 are addressed explicitly rather than counted as false positives.
  They keep their roles, and the distinguishing fact is **which crate serves the claimed role**, a
  manifest fact: `sipx-call` depends on `sipx-media` and `sipx-sdp` but not on `sipx-ua`. For a media
  row the role-serving crate sits above the implementing one and must select it; for a services row
  `sipx-ua` *is* the notifier, nothing above it selects anything, and citing `sipx-call` would mean
  citing a crate that does not depend on it. `packages.rs` is still asserted, but only for what it
  shows — the surface driven from outside its crate — never as the contrast with ICE. The honest
  residual is recorded: the shipped binary cannot subscribe or publish, and if sipx grows a layer
  that *must* be gone through to serve a subscription, these rows acquire the media shape.
  (RFC 5627 is a fifth row of the same shape and is named.)
- Corrected: **8122** (DTLS fingerprint, `implemented`+both roles → `partial`, no roles — the
  `a=fingerprint` branch in `sipx-call` is dead because `Capabilities::with_dtls_srtp` has no
  caller outside `sipx-sdp`'s tests); **8445** and **8839** (ICE, both roles → none —
  `MediaSession::gather` and `start_with_ice` have no caller outside `sipx-media`; `M-27` is the
  wiring story); **3711** (SRTP *is* reachable — evidence now cites
  `crates/sipx-call/tests/secure_media.rs` and `crates/sipx-cli/tests/interop_srtp.rs`).
- The reachable set is derived from the workspace manifests (`call_layer_crates`), not listed —
  measured as `{sipx-call, sipx-cli, sipx-app-protocol}`. Only `crates/…` paths count: the
  repo-root `tests/` escape hatch was removed, since `evidence` may cite markdown and
  `tests/interop/README.md` would otherwise have proved reachability. Verified that **no row relied
  on it** — no evidence path in the registry begins with `tests/`. The residual is recorded rather
  than closed: it is a path test, so `crates/sipx-call/README.md` would still satisfy it. Nothing
  relies on that either — every non-Rust evidence path in the registry is under `docs/specs/`,
  outside `crates/` — and it is filed under "what would widen this" because the successor check
  replaces path matching altogether.
- RFC 3711's note said `secure_media.rs` carries audio "in each direction". It plays **one**:
  `crates/sipx-call/tests/secure_media.rs:86-87` is caller `play` joined to callee
  `record_at_least`, with no reverse leg. Both ends do assert `is_encrypted()` (`:78`, `:81`), and
  the bidirectional evidence is `interop_srtp.rs`, whose `echo_round_trip` is a round trip but which
  is `#[ignore]`d (`:161`) and runs only in the interop matrix. The note now says exactly that —
  this is the one row that kept both roles, in a table published as a measurement.
- Deliberately not done: a cross-crate caller check, which would bind to reachability itself
  rather than to evidence paths, and which would *replace* the layer scope rather than refine it.
  The current check can be satisfied by citing a call-layer file containing a dead branch — that is
  8122's exact shape, and the selection test asserts it (`sipx-call` does render `a=fingerprint`).
- Recorded, not fixed, and the largest hole left: **the check keys on `roles`, not on `status`.**
  `unreachable_role_claims` returns early for a row with no `roles`, so a media row can claim
  `status = "implemented"` for something no call can reach. RFC **6716** and **7587** do exactly
  that — Opus is `implemented` and unreachable: `sipx-call` hardcodes `Capabilities::g711` at
  `call.rs:606`, `:752`, `:955`, `:1728`, `:2860`, `:3161`; `Codec::from_payload_type`
  (`crates/sipx-media/src/session.rs:115-124`) deliberately never returns Opus;
  `Capabilities::with_opus` has no caller outside `sipx-sdp`'s tests; and no `sipx-call` entry point
  accepts caller-supplied `Capabilities` (every mention of the type there is `pub(crate)` or
  private). Found by the coordinator's review and verified here. Filed as `X-33`, deliberately not
  fixed in this story: binding `status` to reachability is a different rule and needs its own
  measurement — this story's own result is what happens when such a rule is adopted unmeasured.
  It also corrected a sentence in the design that claimed proxy and property "agree exactly on
  today's registry"; they agree on the media rows that *claim a role*, which is narrower.
- Recorded, not fixed: **`layer` is author-set**, validated only against `LAYER_TITLE`, so
  relabelling a media row `security` exits the check entirely. It is the strongest argument against
  scoping by layer at all. `test_the_rule_is_scoped_to_the_media_layer` *is* that dodge — it
  relabels a rejected media row and asserts it passes — so the escape is visible in the check's own
  tests rather than only in prose.
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
