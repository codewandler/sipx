---
id: A-13
title: Explain how sipx was built on the public documentation site
pillar: Application
status: done
priority: 11
design:
epic: release
areas: [docs, website]
announcement: 5
note: before A-12; evidence-led development narrative, not an internal story dump
---

# Explain how sipx was built on the public documentation site

## Goal

Give prospective adopters a concise, public account of how the library was implemented and why its
claims are trustworthy: specifications before subsystems, failing-first behavioral tests, bounded
sans-IO cores, executable RFC evidence, generated maturity predicates and one reproducible gate.

## Acceptance

- [x] A curated public page explains the development process from the first protocol boundaries to
      the beta candidate, using RFCs and sipx's own public repository evidence only.
- [x] The page distinguishes engineering method from product capability: it links readers to the
      current compliance, security, CLI and Rust-library pages instead of copying claims that can
      drift.
- [x] Concrete examples show how a spec vector becomes a failing-first test and implementation, how
      the story board preserves decisions between sessions, and how generated checks prevent release
      criteria from becoming prose assertions.
- [x] The account is candid about pre-1.0 change policy, known limits and what the measurements cannot
      prove; it does not imply that test count or RFC percentage is a maturity score.
- [x] The page is reachable from the public navigation, is included in the generated LLM index, and
      the documentation build rejects broken links.

## Progress

- Filed while implementing the `1.0.0-beta.1` roadmap so the release narrative is part of the
  measured public surface, not launch-day copy.
- The public account is an engineering-method page, with links to the live measurements and
  repository evidence instead of transcribing capability claims into another page.
- Added `website/docs/reference/development-process.md` to the Reference sidebar, project footer and
  introduction. The sidebar is also the input to the generated `llms.txt` and `llms-full.txt` files.
- Targeted verification: the production Docusaurus build passed without warnings; its sitemap and
  both LLM files contain the page; the dead-anchor guard still rejects a broken anchor; all 458
  relative documentation links and 17 anchors resolve; and the provenance check is clean. The story
  passed as part of the shared full gate with the rest of the beta wave.

## Notes

- Source material: `docs/vision.md`, `docs/roadmap.md`, `docs/maturity.md`, the gate documentation in
  `AGENTS.md`, story frontmatter/history and representative specs/tests.
- Keep internal mechanics readable to an adopter; do not publish the entire contributor backlog.
