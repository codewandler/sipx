---
id: X-41
title: Make a broken anchor fail the docs build instead of warning inside a green gate
pillar: Build
status: ready
priority: 3
design: docs/roadmap.md
epic: conformance
areas: [docs]
note: alpha predicate 3 — the `docs site` step printed a broken anchor and exited 0, because Docusaurus defaults `onBrokenAnchors` to warn; found by S-30, which only caught it by reading the step's output instead of trusting its exit code
---

# Make a broken anchor fail the docs build instead of warning inside a green gate

## Goal
Make the `docs site` gate step fail when the site it built contains a dead link, so that reading the
step's output is not the only way to learn that it did.

## Acceptance
- [ ] **A broken in-page anchor fails the build.** Docusaurus defaults `onBrokenAnchors` to `warn`, so
      `scripts/build-docs.sh` printed the broken anchor and **exited 0**, and the gate reported 20 steps
      all green with a dead link in the site. Reproduce it first — link to an id the page does not emit,
      such as an `h1`'s implicit id — then set the config so the same link fails.
- [ ] **The other `onBroken*` handlers are audited in the same change**, not just the one that was
      caught. Docusaurus has separate settings for broken links, broken anchors and broken markdown
      links, and they default independently. State the chosen value for each and why; a story that fixes
      only anchors leaves the next reader to discover that links and markdown links were a different
      setting all along.
- [ ] **The gate step's own contract is stated where the step is defined.** `scripts/build-docs.sh`
      already checks inlined samples and internal links and prints counts. It should not be possible for
      it to print a defect and exit 0 — if any of its checks warn rather than fail, that is either
      deliberate and named, or it is this defect again under a different heading.
- [ ] **Failing-first test:** a case that adds a link to a non-existent anchor and asserts the docs
      build exits non-zero. `scripts/test-gate.py` is the existing home for asserting things about the
      gate itself. It must fail before the config change.
- [ ] **No existing anchor breaks.** Turning the setting up will surface every dead anchor already in
      `website/` and `docs/`. Fix them in the same change, or the setting cannot be turned up at all —
      and report the count, because a large number is itself the argument for the story.

## Notes
- **Found by `S-30`**, whose first documentation edit linked to `#cli-reference`, an id Docusaurus does
  not emit for a page's `h1`. The implementor caught it only because it read the step's output rather
  than its exit code, and said so — which is why this is a story rather than a dead link on the site.
- **This is the `X-36` class, not a documentation nit.** `X-36` found a test that was green and could
  not detect the reversal of the invariant it was named for. A gate step that prints a defect and exits
  0 is the same failure: it looks like coverage and is not. The published site is also the thing the
  README points at as a measurement, and the compliance table is served from it.
- **Third instance of gate-integrity drift in one working session**, which is the reason for priority 3.
  `X-39` is a step that *cannot* pass, `X-40` is a test that fails because the machine was busy, and this
  is a step that passes when it should not. All three are alpha predicate 3 — *"a red gate means a
  defect. No test in the workspace fails because the machine was busy"* — which the roadmap calls
  load-bearing for the other six, since every predicate is asserted by the gate. The three failure modes
  are different and the consequence is the same: the gate's verdict stops meaning what it says.
- **It is a shared config file**, which is why `S-30` correctly left it alone rather than widening its
  own diff. That makes it a one-line change plus whatever cleanup the setting surfaces — cheap unless
  the site has accumulated dead anchors, which is exactly what the last Acceptance item is for.
