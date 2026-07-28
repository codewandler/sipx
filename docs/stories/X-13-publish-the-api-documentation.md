---
id: X-13
title: Publish the API documentation
pillar: Build
status: backlog
priority: 3
design:
epic: docs-site
areas: [docs]
note: blocked by X-8
---

# Publish the API documentation

## Goal
`cargo doc` for the whole workspace, on the site, so the library half of sipx is usable without
cloning it.

## Acceptance
- [ ] `cargo doc --workspace --all-features` published alongside the guides.
- [ ] Documentation warnings are denied, so a missing doc on a public item fails the build.
      Every public item in this workspace already has one; the point is to keep it that way.
- [ ] Intra-doc links resolve, checked rather than assumed.
- [ ] The guides link into the API docs at the types they discuss.

## Progress
- Not started. Blocked by `X-11`.
