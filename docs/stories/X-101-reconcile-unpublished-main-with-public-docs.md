---
id: X-101
title: Reconcile unpublished main with the public docs
pillar: Build
status: done
priority: 4
design:
epic:
areas: [website, docs]
predicate:
announcement:
note: audit every shipped capability in origin/main..main against the public adoption surface and remove stale denials
---

# Reconcile unpublished main with the public docs

## Goal

Compare the complete unpublished `origin/main..main` history with the public README, crate landing
pages and website, then make every newly shipped user-facing capability discoverable without
presenting unreleased work as part of the immutable beta.4 release.

## Acceptance

- [x] The audit covers every completed story changed in `origin/main..main`, grouping commits by the
      capability they deliver rather than treating merge and review commits as separate features.
- [x] Each shipped capability is either already represented by a current public page or gains one
      concise adoption-path entry; internal-only checks and test hardening are recorded as needing no
      separate public promise.
- [x] The current fit guide no longer says in-dialog `MESSAGE` lacks user-agent behavior after the
      application-owned dialog-request API shipped, and a failing-first public-content regression
      prevents that denial from returning.
- [x] Public library guidance covers live TLS identity rotation, bounded endpoint observation,
      request policy, source admission, and application-owned INFO/MESSAGE/private dialog methods.
- [x] The intro, fit guide, application-host overview and post-beta changes page agree that the
      realtime binding is implemented on `main`, while the credentialed live-endpoint proof remains
      pending and no tagged-release claim is widened.
- [x] The generated comparison is visually inspected at desktop and phone widths; dense evidence
      tables remain inside the article, preserve readable columns and advertise horizontal scrolling
      on narrow screens.
- [x] `sync-website.py --check`, public-content tests, documentation links/build, provenance, maturity
      and formatting are green.

## Progress

- 2026-08-05: audit window fixed at merge base `811e688` through current `main` (85 unpublished
  commits at filing). The initial commits collapse into the M13 endpoint wave, the realtime-agent
  bridge, and one working-agreement-only maintenance change.
- 2026-08-05: existing public coverage is complete for registration observation and discovery,
  inbound/outbound event services, publication, dialog snapshots, deterministic call testing, RTP
  echo, logging policy, the bounded load responder, comparison data and realtime bridge details.
  Gaps found: T-31/T-32 have no public adoption prose; S-40 contradicts the fit guide's MESSAGE
  denial; the intro/fit host summaries omit the realtime binding; and What's new names no delivered
  work after beta.4 despite explicitly documenting `main`.

## Audit

| Delivered change group | Public evidence after reconciliation |
|---|---|
| T-31, T-32 — identity rotation and bounded endpoint seams | `sipx-transport` README; fit, library, integration and current-main guides |
| S-35, S-37, S-38, S-39, S-24 — notifier, subscriber, publication and registration discovery | library and fit guides; registration and CLI references; generated RFC compliance |
| S-40 — application-owned dialog requests | `sipx-call` README; library, fit, integration and current-main guides; stale-denial guard |
| S-42 — registration observation | registration guide and generated RFC compliance |
| S-43 — confirmed-dialog snapshots | README/intro crate tables, library guide and dedicated persistence guide |
| X-75, M-53 — deterministic call testing, logging and RTP echo | `sipx-testkit` README; call-test, RTP-echo and logging guides; library/current-main guides |
| P-15, X-98, X-99 — bounded responder, neutral load contract and first result | CLI reference, generated public comparison, hashed evidence registry and current-main guide |
| X-97 — evidenced capability inventory | public generated comparison and its adoption-path links |
| X-38 — production application reachability | README stability boundary, host overview and generated app-surface report |
| A-19 through A-22 — realtime contract, client, peer and routed bridge | app/testkit READMEs; SDK overview; intro, fit, integration, library and current-main guides |
| Review, merge and hardening commits | No separate feature claim; they tighten the bounds and evidence already stated above |
| Backlog-only A-16–A-18, M-52, S-41, T-33 and X-100 edits | No shipped claim; current pages continue to state those browser SDK boundaries as absent or pending |

- 2026-08-05: the stale-claim guard was seen failing on the old MESSAGE denial before the public
  pages changed. All 23 synchronization tests, generated-region checks, front-door and app-surface
  checks, provenance, internal link/anchor checks, static-site build and API-reference build pass on
  the reconciled tree.
- 2026-08-05: while the audit was running, `main` fast-forwarded by one commit carrying X-99's
  complete comparative-load dataset and generated public result. The audit expanded to all 86
  unpublished commits, preserved those independently landed files, and replaced its now-stale
  "result pending" sentence with a link and the result's own non-ranking scope limits.
- 2026-08-05: headless-browser renders at 1440 px and 390 px reproduced the comparison page's
  compressed, extremely tall evidence rows. The final render keeps short tables fluid and constrains
  each dense table to the article with horizontally scrollable, purpose-sized columns; both viewport
  widths were inspected after the static-site rebuild.
