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
- [ ] **An application exists that is not the test suite and not the CLI.** `sipx-app` is the host
      (`crates/sipx-app`), and the `A-*` epic tracks it. This is the piece v1 predicate 3 already asks
      for — *"the public API has been used from outside this repository"* — so this story and that
      predicate land together or not at all.
- [ ] **The reachable-from-a-call surface is stated as "what the application uses", not "what a grep
      found".** The difference matters: `X-30` and `X-33` both shipped checks that read evidence
      *paths*, and both documented that a path can be satisfied by citing a file whose relevant branch
      is dead. An application has no dead branches it can cite — either it builds and runs on the
      API, or it does not.
- [ ] **`docs/maturity.md` reports predicate 1 against this definition**, and the "unverified against
      callers" caveat on `core`, `services`, `transport` and `wire` is resolved by it, not by a
      per-layer check. A caveat resolved by reality ages better than one resolved by a rule.
- [ ] **Anything the application does not use is marked experimental**, following `A-8`'s rule, and the
      list is non-empty. A shipped app that needs everything is a claim and should be checked like one.
- [ ] **A second implementation disagreeing widens the surface.** The rule must say what happens when
      something outside the repo depends on an experimental item: it graduates, with a changelog entry.
      Without that the definition is a freeze, not a measurement.
- [ ] Failing-first test: name the assertion that fails while the reachable surface and the
      application's actual dependencies disagree.

## Progress
- Not started. Filed at `X-37`'s close, which reconsidered the predicate rather than build the check
  its predecessors named as a *successor* — read its Notes for why.

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
