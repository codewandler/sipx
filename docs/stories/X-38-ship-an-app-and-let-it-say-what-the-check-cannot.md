---
id: X-38
title: Ship a real application, and let its reality say what a check cannot
pillar: Build
status: in-progress
priority: 4
design: docs/vision.md
epic: app-sdk
areas: [sipx-app, docs]
note: alpha predicate 1, reconsidered at X-37 — a syntactic caller-check would be fitted to three rows, wrong in the ways macros and re-exports are wrong, and fitted to what is testable rather than what is true; the honest gate is that the reachable-from-a-call surface is exactly what one real application uses
---

# Ship a real application, and let its reality say what a check cannot

## Goal
Make alpha predicate 1 true by the one check that cannot be gamed: an application nobody wrote to
pass a checker, whose every dependency the workspace can see. The reachable-from-a-call surface is
then *defined* as exactly what that application uses, and anything it does not use is experimental
until a second application disagrees.

## Acceptance
- [x] **An application exists that is not the test suite and not the CLI.** `sipx-app` is the host
      (`crates/sipx-app`), and the `A-*` epic tracks it. This is the piece v1 predicate 3 already asks
      for — *"the public API has been used from outside this repository"* — so this story and that
      predicate land together or not at all.
      → `crates/sipx-app/src/host.rs` (`Host::run`) and the `sipx-host` binary
      (`crates/sipx-app/src/bin/sipx-host.rs`). Asserted by
      `test-app-surface.py::test_the_application_is_neither_the_test_suite_nor_the_cli`.
- [x] **The reachable-from-a-call surface is stated as "what the application uses", not "what a grep
      found".** The difference matters: `X-30` and `X-33` both shipped checks that read evidence
      *paths*, and both documented that a path can be satisfied by citing a file whose relevant branch
      is dead. An application has no dead branches it can cite — either it builds and runs on the
      API, or it does not.
      → `scripts/check-app-surface.py`: the used side is the application's Cargo dependency closure,
      not a path list. `[dev-dependencies]` are excluded, so a test cannot widen the surface.
- [x] **`docs/maturity.md` reports predicate 1 against this definition**, and the "unverified against
      callers" caveat on `core`, `services`, `transport` and `wire` is resolved by it, not by a
      per-layer check. A caveat resolved by reality ages better than one resolved by a rule.
      → `scripts/maturity.py`: predicate 1 is now `computed`, the layer column is *Reachability
      basis*, and `REACHABILITY_CHECKED` is deliberately **not** widened.
- [x] **Anything the application does not use is marked experimental**, following `A-8`'s rule, and the
      list is non-empty. A shipped app that needs everything is a claim and should be checked like one.
      → seven `**Experimental** (`A-8`)` modules plus `sipx-app-protocol`, which no application
      reaches. Non-emptiness is asserted, not assumed:
      `test-app-surface.py::test_an_application_that_needs_everything_is_reported`.
- [x] **A second implementation disagreeing widens the surface.** The rule must say what happens when
      something outside the repo depends on an experimental item: it graduates, with a changelog entry.
      Without that the definition is a freeze, not a measurement.
      → `README.md` §Crates, and the same clause in `crates/sipx-app/src/lib.rs`'s `# Stability`.
      Mechanically, `APPLICATIONS` is a tuple: a second root joins it rather than being refused.
- [x] Failing-first test: name the assertion that fails while the reachable surface and the
      application's actual dependencies disagree.
      → `./scripts/check-app-surface.py --check`, whose assertion is
      `unreached_supported`. At the merge base it names six crates — `sipx-audio`, `sipx-call`,
      `sipx-media`, `sipx-rtp`, `sipx-sdp`, `sipx-ua` — that declare `Supported` surface no
      application reaches; after the host exists it names none.

## Progress
- **Done, pending review.** The host exists (`crates/sipx-app/src/host.rs`, `sipx-host` binary), the
  surface is derived from its dependency closure (`scripts/check-app-surface.py`), the maturity report
  reports predicate 1 against that definition, and the graduation rule is in `README.md`.
- **What the host deliberately does not do.** It runs no app callback: `A-2`, `A-4` and `A-5` are all
  still open, so nothing carries `sipx.app.v1` to customer code yet. That absence is routed through the
  document's own §9.2 `on_unreachable` declaration rather than papered over, which is also the first
  time a `FailurePolicy` knob decides something outside the harness.
- **It answers OPTIONS through `sipx_ua::UserAgent`** (RFC 3261 §11), and that is not decoration. The
  surface check demanded it: `sipx-ua` declares registration and digest auth *Supported*, and with the
  host reaching only `sipx-call` the check correctly reported that no application reached `sipx-ua`.
  The choice was to demote a crate the CLI genuinely exercises or to have the host use it for something
  it genuinely needs — an unanswered liveness probe is a host a carrier marks down.
- **Two bugs in the checker, both reporting nothing**, are now regression tests. A multiline pattern
  whose `\s` crossed a newline read every crate's stability glossary as a claim; and a substring test
  for `**Supported**` skipped the four crates that write `**Supported.**` with the period inside the
  emphasis. A checker with no output is indistinguishable from a clean tree, which is why both are
  pinned.
- **`test_the_report_states_its_blind_spot` was passing for the wrong reason** and was repinned. It
  asserted the string `unverified against callers`, which the replacement bullet quotes while saying it
  is gone — the `X-36` defect exactly, so it now asserts the new limit instead.
- **Known limit, stated rather than left to be found.** Assertion 1 runs at crate granularity, because
  that is the granularity the `# Stability` declarations have. A supported *module* that nothing in the
  closure names is not caught. Per-module declarations would let it tighten; that is a successor and
  not something to fake by parsing English.
- Filed at `X-37`'s close, which reconsidered the predicate rather than build the check its
  predecessors named as a *successor* — read its Notes for why.

## Notes
- **Why `X-37` filed this instead of the caller-check.** Both `X-30` and `X-33` said the cross-crate
  caller check was the successor — and both said it **in prose, after building the path check**, which
  is the one moment building the *next* check is most tempting and least examined. The caller check
  takes a different input, and the cheap version of it is fitted to the three rows that motivated it
  (5626, 8599, 8122): the exact "rule fitted to the data it was tested on" failure this story's whole
  lineage keeps warning about. The accurate version is a dependency plus minutes on the gate, for a
  return a grep already proved to be two honest demotions.
- **The pattern is now eight for eight, and it has a name: a capability that exists in a crate and
  cannot be selected from a call.** A grep is enough to find the *next* one. What a check cannot tell
  you is whether a capability is *worth* selecting — only an application can. The `transport` layer's
  selected-vs-plumbing mix is not a taxonomy problem to solve; it is the same question, and the
  application answers it by existing.
- **This is not a retreat from mechanical checking.** The registry check, the front-door guard, the
  maturity report and the stability rule all stay, and they are all mechanical. The claim here is
  narrower: that *this particular* predicate — "no claim outlives its caller" — is about a property
  of use, and use is observed by shipping something that uses it. v1 predicate 3 says the same thing
  in its own words.
- **The two rows `X-37` demoted are not waiting on this.** `S-29` wires Outbound and push to a call,
  which makes RFC 5626 and 8599's `uac` roles honest by the ordinary route. This story is about the
  *layer* question those rows happened to expose, not about those rows.
- Reads with `A-8` (the experimental rule this leans on), `X-32` (the maturity report that must
  change its basis), and the `A-*` epic (the application itself).
