---
id: X-33
title: Generalise the reachability check past the media layer
pillar: Build
status: ready
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
- [ ] Still **no suppression list**, under any name. `X-30` held that line and it is the reason the
      check is worth having.
- [ ] Failing-first test: a fixture row at a non-media layer claiming a role reachable from nothing,
      passing `--check` today. Name the test that makes it fail.

## Progress
- Not started.

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
