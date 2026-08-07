---
id: published-adoption
---

# Executable published adoption path

**Status:** proposed · **Pillar:** Build · **Epic:** `published-adoption` ·
**Review:** [external full-sweep review](../reviews/extern-2026-08-06T01-56-26+02-00-full-sweep.md)
findings 6 and 7 · **Stories:** `X-111`

## Problem

The published onboarding path is useful only when its dependency list compiles the example it
introduces and its generated release version renders as ordinary prose. The follow-up review found
both seams broken: the answer-a-call example imports a package absent from the dependency snippet,
and an inline generated marker splits and truncates the version sentence on the public site.

## Direction

One archived consumer fixture owns the dependency snippet and example together. Documentation
includes or derives from that executable source instead of maintaining an uncompiled second copy.
Generated scalar values occupy a markdown-safe shape: either a standalone generated block or a
site-generator-supported inline substitution, never an HTML block marker embedded in prose.

## Exit

A clean registry-shaped consumer compiles every published getting-started example from exactly the
dependencies shown beside it, and the built public site contains the complete workspace version in
one readable sentence. Checks fail when either source drifts.
