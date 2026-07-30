---
id: X-41
title: Make a broken anchor fail the docs build instead of warning inside a green gate
pillar: Build
status: in-progress
priority: 3
design: docs/roadmap.md
epic: conformance
areas: [docs]
predicate: 3
note: alpha predicate 3 — the `docs site` step printed a broken anchor and exited 0, because Docusaurus defaults `onBrokenAnchors` to warn; found by S-30, which only caught it by reading the step's output instead of trusting its exit code
---

# Make a broken anchor fail the docs build instead of warning inside a green gate

## Goal
Make the `docs site` gate step fail when the site it built contains a dead link, so that reading the
step's output is not the only way to learn that it did.

## Acceptance
- [x] **A broken in-page anchor fails the build.** Docusaurus defaults `onBrokenAnchors` to `warn`, so
      `scripts/build-docs.sh` printed the broken anchor and **exited 0**, and the gate reported 20 steps
      all green with a dead link in the site. Reproduce it first — link to an id the page does not emit,
      such as an `h1`'s implicit id — then set the config so the same link fails.
- [x] **The other `onBroken*` handlers are audited in the same change**, not just the one that was
      caught. Docusaurus has separate settings for broken links, broken anchors and broken markdown
      links, and they default independently. State the chosen value for each and why; a story that fixes
      only anchors leaves the next reader to discover that links and markdown links were a different
      setting all along.
- [x] **The gate step's own contract is stated where the step is defined.** `scripts/build-docs.sh`
      already checks inlined samples and internal links and prints counts. It should not be possible for
      it to print a defect and exit 0 — if any of its checks warn rather than fail, that is either
      deliberate and named, or it is this defect again under a different heading.
- [x] **Failing-first test:** a case that adds a link to a non-existent anchor and asserts the docs
      build exits non-zero. `scripts/test-gate.py` is the existing home for asserting things about the
      gate itself. It must fail before the config change.
- [x] **No existing anchor breaks.** Turning the setting up will surface every dead anchor already in
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

## Progress

**The dead-anchor count is zero.** The last Acceptance item asked for it because a large number would
itself be the argument for the story; it is not large. The published site has exactly **one** anchor
link (`website/docs/intro.md` → `#the-honest-version`) and it resolves, so turning `onBrokenAnchors`
up needed no cleanup at all. The internal tree has **six** anchor links across 192 pages, all of them
live. `S-30`'s `#cli-reference` was the only dead one and `S-30` had already fixed it. The story is
carried entirely by the second and third Acceptance items — the setting, not the cleanup.

What was done:

- `website/docusaurus.config.js` — all **four** reporting handlers now stated with a reason each,
  where two were inherited before. `onBrokenAnchors: 'throw'` is the defect. `onDuplicateRoutes:
  'throw'` was audited in with them: it is not a link defect, but it defaulted to `warn` and so had
  the identical shape — printed, and exited 0. `onBrokenLinks` already defaulted to `throw` and is now
  stated so a reader can tell a decision from an absent line; `onBrokenMarkdownLinks` was already
  `throw` under `markdown.hooks`, and is now audited rather than being the one setting somebody
  remembered.
- `scripts/build-docs.sh` — the step's contract is written at the top as a table of what it checks and
  how each one fails, under one rule: **no check in this file may print a defect and exit 0.** Two new
  routes closed. The site build's output is captured and any `[WARNING]` fails the step, with an
  intentionally empty `WARNING_EXCEPTIONS` list as the named place for a deliberate exception — so a
  fifth handler in a later Docusaurus cannot ride green under a heading nobody here has read. And the
  step now proves the guard is *armed* rather than trusting the config: it writes a page linking to
  the front page's `h1` id (the exact `S-30` shape — Docusaurus renders the first `h1` as the page
  header and gives it no id) and fails if that build succeeds. It reuses the warm build cache, so it
  costs about 6 s.
- `scripts/check-docs-links.py` — new, and it is the internal-links heredoc lifted out of
  `build-docs.sh` and extended to anchors. The heredoc did `link.split("#")[0]`: a link to a missing
  *file* failed the build and a link to a missing *heading* was invisible. Extracting it also makes it
  testable on fabricated trees, which the heredoc never was.
- `scripts/test-gate.py` — `TheDocsSiteStep` (the four handler values, each with a reason, the
  deprecated top-level spelling, and that the probe check and the warning guard are still in the step)
  and `TheInternalDocsLinkCheck` (the anchor resolver, on trees the test writes).

### Rework round 1 — the probe was a gate-integrity defect of its own

The review was right and it is the ugliest kind of finding: the check written to defend
`predicate: 3` could violate it. The probe page lands in `website/docs/`, a tracked directory,
because that is the only place Docusaurus builds pages from — and a `kill -9` runs no `trap`. Two
consequences, both reproduced by the reviewer:

- Nothing ignored it, so `git add -A` after a killed gate run would have committed a page with a
  dead anchor into the published site. The integrating agent is the `git add -A`.
- The cleanup could not run in the case it existed for. It sat *after* the real site build, and a
  leftover probe makes that build fail — correctly, it is a dead anchor in the tree. Under `set -e`
  the script aborted there and never reached the cleanup, so the gate stayed red on every later run
  with no defect in the tree, until a human deleted a file `git status` was about to stop showing
  them.

Fixed by placement rather than by more cleanup:

- The stale-probe sweep is now the first thing the script does, before anything reads
  `website/docs`. A probe from a killed run is gone before the build that would trip on it.
- The path is in the root `.gitignore`, with the reasoning, beside the three other entries there
  for this class of file. Asserted through `git check-ignore` rather than by matching the pattern,
  because the property is "`git add -A` cannot stage it", not "a line exists".
- The name carries `$$`, so two runs in one checkout cannot delete each other's live probe. The
  sweep is a glob; the end-of-run removal is this run's own path.

A throwaway copy of `website/` would have kept the probe out of the tree altogether, and it was
tried: **2m31s** for the cold cache, and it failed for the wrong reason. Not worth it on every gate
run against a fix that is three lines and a `.gitignore` entry.

Also from the review, `slug()` diverged from GitHub on two shapes present in the tree today —
`_emphasis_` left unconsumed (3 headings in `docs/roadmap.md`) and `<name>` inside a code span
eaten as an HTML tag because backticks were stripped first (4 headings in
`docs/specs/host-config.md`). Neither could fire yet, because none of the 7 is linked. Fixed, and
the fix is not an ordering tweak: code spans are held out of every markup rule and restored just
before the punctuation strip, so "a code span is literal" survives whatever rule is added next.
There is a test over the 7 real headings as well as the fixtures.

Still not addressed, deliberately: two concurrent `build-docs.sh` runs in **one checkout** collide
on `website/build` and `website/.docusaurus`, which they did before this story and still do. The
probe no longer adds to that. Cross-worktree is unaffected — each has its own path.

One trap for whoever touches the slug rule: spaces map to hyphens **one for one, not run for one**.
The `Application SDK` heading in `docs/roadmap.md` carries an em dash, and the em dash is dropped
while both spaces around it survive — so it anchors as `application-sdk--app-sdk`, with two hyphens.
A slugger that collapses whitespace runs reports the two live links at `docs/roadmap.md:108` as dead,
which is how this was found; there is a regression test for it.

Not done, deliberately: the real end-to-end "a dead anchor exits non-zero" case lives in
`build-docs.sh`, not in `test-gate.py`. CI's `gate` job has no node, so a case there would have had to
skip itself on every machine but a developer's — and a check that skips where it matters is the
disease. `test-gate.py` asserts the decision the step obeys and that the step still makes it.

The gate's `maturity` step is red on this branch **and on its merge base**, `36d0b3f`, with a pristine
tree: that commit closed `M-31` and regenerated `docs/maturity.md` in the same commit, so the
burn-down's "closed today" count in the committed file is one short of what the same commit's history
renders. That is `X-39` exactly — *the maturity report cannot be green in the commit that moves a
story*. Nothing in this diff touches it, the only fix is regenerating a file this story is fenced out
of, and reverting this story's own `status:` does not change the number, because it is read from git
history rather than from frontmatter. Left for whoever sequences `X-39`.
