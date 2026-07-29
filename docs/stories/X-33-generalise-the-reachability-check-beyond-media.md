---
id: X-33
title: Generalise the reachability check past the media layer
pillar: Build
status: in-progress
priority: 3
design: docs/designs/rfc-registry-grain.md
epic: conformance
areas: [docs]
note: alpha predicate 1 — X-30 made "no claim outlives its caller" mechanical for layer = media only, and its own review showed the reason given for stopping there was false
---

# Generalise the reachability check past the media layer

## Goal
Make alpha predicate 1 true at every layer: no registry entry claims a role that nothing above the
implementing crate can reach.

## Acceptance
- [ ] The reachability check in `scripts/rfc-report.py` applies beyond `layer = "media"`, or the
      restriction is re-argued on evidence and recorded as a **choice** rather than as a structural
      necessity. `X-30` measured the unscoped rule at 22 of 29 role-claiming rows rejected with only
      3 just, which is a real result — but its stated reason for stopping ("seven `sipx-ua` rows
      cannot satisfy it at any price") was **false**, and its own review proved it.
- [ ] The four `sipx-ua` presence rows — **3680, 3856, 3903, 4235** — are resolved individually.
      Nothing above `sipx-ua` calls the presence/publish path, so under `X-30`'s own thesis these
      are the media over-claims' shape one layer over. Each is demoted, given an honest citation, or
      given a written reason it differs. Counting them as false positives without argument is the
      "rule fitted to the data it was tested on" failure.
- [ ] **`sipx-cli` is treated as what it is: the crate that sits above both `sipx-call` and
      `sipx-ua`** (`crates/sipx-cli/Cargo.toml:21-22`). `X-30`'s design filed "if an application
      crate came to sit above both" as a *future* widening trigger; it had already fired. Whatever
      scope this story lands on, the design must stop describing that condition as pending.
- [ ] The two escape hatches `X-30`'s review found are closed or recorded: the unqualified repo-root
      `tests/` path (`scripts/rfc-report.py:127` — `tests/interop/README.md` currently satisfies
      reachability), and `layer` being author-dodgeable, since it is validated only against
      `LAYER_TITLE` (`:207`) so relabelling a media row `security` exits the check entirely.
- [ ] **The check gates on `roles`, not on `status` — so a row can claim `status = "implemented"`
      with nothing above it reachable and never be interrogated.** Verified at integration by reading
      the registry directly: RFC 6716 and 7587 are `status = "implemented"`, `layer = "media"`, and
      carry **no `roles` field at all**. Opus is unreachable from any call — `sipx-call` hardcodes
      `Capabilities::g711` (`call.rs:606,752,955,1728,2860,3161`), `Codec::from_payload_type`
      (`sipx-media/src/session.rs:115-124`) *deliberately* never returns Opus, `with_opus` has no
      caller outside `sipx-sdp`'s own tests, and no `sipx-call` entry point accepts caller-supplied
      `Capabilities`. This is the media over-claim shape inside the layer the check already covers,
      escaping through a different field. Decide whether `implemented` implies reachability; if it
      does, this is the third escape hatch.
- [ ] **RFC 6665's note is stale in the gated artifact.** `docs/rfc/registry.toml:331` and
      `docs/compliance.md:74` both end "no event packages ship yet (`S-17`, `S-18`)". Both stories are
      `status: done`, the packages are public API (`sipx-ua/src/packages.rs:107` → `"dialog"`, `:224` →
      `"reg"`, plus `presence.rs`), rows 3680/3856/4235 in the same table describe them as shipped, and
      the website sells them. `README.md:38` calls this table "a measurement rather than a claim";
      this sentence is neither.
- [ ] Still **no suppression list**, under any name. `X-30` held that line and it is the reason the
      check is worth having.
- [ ] Failing-first test: a fixture row at a non-media layer claiming a role reachable from nothing,
      passing `--check` today. Name the test that makes it fail.

## Progress
- Not started. Two items were added after filing, from `X-30`'s rework and from a read-only public-docs
  sweep: the `roles`-not-`status` hole (Opus) and RFC 6665's stale note. `X-30`'s implementor was told
  to record the first as a known limit of its check rather than widen its own scope at the end of a
  rework, so expect a paragraph in `docs/designs/rfc-registry-grain.md` naming it — that paragraph is
  this story's starting point, not a substitute for it.
- **`X-30`'s scope argument survived its rework and should be inherited, not re-litigated.** The
  honest version is the one this story's Notes predicted: media is where a capability is *selected*
  (`with_srtp`, `with_dtls_srtp`, `start_with_ice`), and selecting nothing is the silent default — the
  call still connects and every test in the crate below still passes. There is no `with_transactions`
  and no `with_dns`, so "can a call reach the transaction layer" cannot come out `no`. `layer =
  "media"` is a **proxy** for selection, and the proxy was tested against the property.
- **Twice in `X-30` a mechanically appealing untrue fact stood in for a judgement**, which is worth
  knowing before starting here. First: "seven `sipx-ua` rows cannot satisfy it at any price" — false,
  `sipx-cli` sits above both `sipx-call` and `sipx-ua`. Then its own replacement: `packages.rs` proves
  a cross-crate caller "exactly what `start_with_ice` has none of, in any crate, including their own
  integration tests" — also false, `crates/sipx-media/tests/ice.rs:149,150` calls it, and had that
  been the criterion 8445 and 8839 would have passed and the story's central correction would have
  collapsed. Argue from a property, then test the proxy against it.

## Notes
- **Alpha predicate 1** (`docs/roadmap.md`, "The v1 gate"). The predicate is deliberately stated as
  *any* layer, because the pattern that produced it was never media-specific — it was found in ICE,
  in UPDATE (`S-22`, a `core` row), in DTLS-SRTP, and in RFC 4568's answer check.
- **The honest version of `X-30`'s argument is available and probably survives**: media capabilities
  are *selected* by the call layer, and selecting nothing is exactly how ICE and DTLS-SRTP shipped
  unreachable, whereas the transaction layer and DNS are on the path every call already takes. That
  is a property-based reason to scope by *selection* rather than by the string `media` — worth
  testing as the rule before defaulting to a layer list.
- A **cross-crate caller check** would bind to reachability itself rather than to evidence paths,
  which is the deeper fix `X-30` recorded as "what would widen this". The current check can be
  satisfied by citing a call-layer file containing a dead branch — RFC 8122's exact shape before it
  was demoted. Worth scoping here or splitting out, but not worth pretending the path check is
  equivalent.
