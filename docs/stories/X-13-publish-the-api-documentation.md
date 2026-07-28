---
id: X-13
title: Publish the API documentation
pillar: Build
status: done
priority: 6
design:
epic: docs-site
areas: [docs]
note: track: docs · after X-12 in the same crate-free track
---

# Publish the API documentation

## Goal
`cargo doc` for the whole workspace, on the site, so the library half of sipx is usable without
cloning it.

## Acceptance
- [x] `cargo doc --workspace --all-features` published alongside the guides.
- [x] Documentation warnings are denied, so a missing doc on a public item fails the build.
      Every public item in this workspace already has one; the point is to keep it that way.
- [x] Intra-doc links resolve, checked rather than assumed.
- [x] The guides link into the API docs at the types they discuss.

## Progress
- Done. `scripts/build-docs.sh` builds `cargo doc --workspace --all-features` into
  `target/book/api/` so it deploys with the guides, and CI runs the same script.
- **`RUSTDOCFLAGS="-D warnings"`, plus `missing_docs` and the `rustdoc` lint group in the
  workspace lints.** Verified by deleting a doc comment and confirming the build fails, because
  a gate that reports a problem without failing is not a gate — the same thing that had to be
  fixed in `check-features.sh` and again in the include check.
- **The story's premise was wrong**: it said "every public item in this workspace already has
  one". Turning the lint on found `MediaSession::stats` undocumented and two ambiguous
  intra-doc links (`[\`answer\`]` and `[\`crate::resolve\`]`, each both a function and a module
  in its own crate — rustdoc cannot guess, and the link would have silently gone to the wrong
  one). All three fixed.
- rustdoc writes no index at the root of a multi-crate build, so `/api/` alone would 404. There
  is a small redirect page pointing at `sipx-call`, which is where a reader most likely wants
  to start.
- The link checker skips the generated pages — rustdoc has already checked its own intra-doc
  links with `-D warnings`, and re-walking 471 pages would add a minute to every docs build to
  answer the same question twice. Links *into* the API from the guides are still checked, and
  that immediately caught two wrong paths: `sipx_sdp::answer` and `Capabilities` live under an
  `answer` module, not at the crate root.
