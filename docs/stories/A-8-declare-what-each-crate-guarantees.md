---
id: A-8
title: Declare what each published crate guarantees
pillar: Application
status: ready
priority: 3
design: docs/vision.md
epic: app-sdk
areas: [docs, sipx-app-protocol]
note: alpha predicate 5 — v1 freezes what "stable" means, so the line between stable and experimental has to exist before it can be frozen
---

# Declare what each published crate guarantees

## Goal
Give every published crate an explicit, written statement of which of its public surface is
**stable** and which is **experimental**, so cutting `1.0.0` freezes something that was decided
rather than something that merely happened to compile.

## Acceptance
- [ ] Each published crate states its guarantee in its crate-level documentation, where a reader on
      docs.rs meets it first — not only in a repository markdown file. Eleven crates publish;
      `sipx-testkit` is `publish = false` and needs no promise.
- [ ] **The unit of the promise is stated.** "Stable" must say what may still change: new enum
      variants behind `#[non_exhaustive]`, new struct fields, new trait methods with defaults. This
      project has already shipped three additive `sipx_call::Error` variants that were
      source-breaking for an exhaustive `match`, and said so in the changelog each time — that
      practice becomes the written rule.
- [ ] **Experimental surface is marked at the item, not only in prose.** `sipx-app-protocol` already
      describes itself as experimental; the rule is that a reader looking at one type can tell
      without going up a level.
- [ ] Anything that cannot honestly be called stable before 1.0 is named, with what would settle it.
      An empty list here is a claim, and by this project's standards claims get checked.
- [ ] The declaration is reachable from the README's crate table, so the question "can I depend on
      this" is answered where people ask it.
- [ ] Failing-first evidence: name a crate whose public surface a reader cannot today classify as
      stable or experimental from its own documentation — and the assertion, test or gate step that
      makes it classifiable.

## Progress
- Not started.

## Notes
- **Alpha predicate 5** (`docs/roadmap.md`, "The v1 gate"). It is the one alpha item that is pure
  decision rather than correction, and the reason the alpha exists at all: **cutting `1.0.0` freezes
  the public API, and this API has not yet been used by anyone outside this repository.** An alpha
  release is how that gets exercised before the freeze.
- Related: the `sipx.app.v1` contract already carries a version in its name and is marked
  experimental, matching its spec's status. That is the model — the promise is written down and the
  version says which promise.
- **Do not turn this into a semver policy essay.** One paragraph per crate that a person deciding
  whether to depend on it can act on. The vision's non-goals discipline applies to documentation as
  much as to features.
- Worth checking against `docs/rfc/registry.toml` while writing: a crate that publishes a capability
  the registry marks `partial` should not describe that surface as stable without saying which half
  works — the same honesty rule `X-30` and `M-28` applied to the table.
