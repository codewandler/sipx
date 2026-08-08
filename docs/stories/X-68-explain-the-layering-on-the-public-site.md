---
id: X-68
title: Explain the layering on the public site
pillar: Build
status: done
priority:
design: docs/designs/docs-depth.md
epic: docs-depth
areas: [website]
predicate:
announcement:
note: sans-IO is the central design property and the site never states it · one page, one diagram · beta-1
---

# Explain the layering on the public site

## Goal

Give a prospective user one page that explains how sipx is layered and why, so the property that
makes the core fuzzable, deterministic and runtime-free is discoverable by someone who is not
reading `AGENTS.md`.

## Acceptance

- [x] A concepts page exists in the site's Start section explaining the sans-I/O boundary: which
      crates hold no socket, no runtime and no clock read; how time enters as a fired-timer input and
      bytes as data; where the seam to the driver crates falls; and which crate a reader reaches for
      to do a given job.
- [x] It states what the property buys rather than only asserting it — parser and transaction fuzzing,
      virtual-time determinism in the harness, and a core testable with no network.
- [x] It carries one Mermaid layer diagram that renders legibly in both the light and dark site
      themes. Mermaid is already enabled; no new asset pipeline is introduced.
- [x] The page's architectural claims are held at the level `AGENTS.md` non-negotiable 2 already pins,
      so a violation fails review against the non-negotiable rather than only against this page.
- [x] It is reachable from the sidebar before the library guides, and `build-docs.sh` passes with no
      new entry in `WARNING_EXCEPTIONS`.
- [x] `./scripts/gate.py` green.

## Progress
- 2026-08-05: selected for the post-beta.7 foundations and field-hardening wave. The first
  failing-first docs build names the new Start-page route before the page exists; implementation
  then has to supply the architecture contract rather than leaving an untested orphan document.
- 2026-08-05: the Architecture page now names the core/driver seam, explicit byte and timer inputs,
  crate selection, fuzzing and virtual-time consequences, with one Mermaid layer diagram. It is in
  Start before the guides; the complete docs build and the final production-site build are green
  with no warning exception.

## Notes
- The 2026-08-04 capability review found the site has exactly one Mermaid diagram in ~21,500 words,
  and no concepts or architecture page at all. The layering is stated in `AGENTS.md`, in crate
  `lib.rs` headers and in `docs/specs/` — all contributor-facing surfaces.
- Deliberately does not teach SIP itself. That is the larger deferred item in
  [`docs/designs/docs-depth.md`](../designs/docs-depth.md), and it competes with making shipped
  features discoverable (`X-69`), which wins first.
- No `announcement:` field: this page makes the surface *better*, not more honest, and predicate 5
  means honest and current. Do not inflate it.
