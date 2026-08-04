---
id: X-72
title: Define the comparison data model and its checker
pillar: Build
status: done
priority: 15
design: docs/designs/stack-comparison.md
epic: stack-comparison
areas: [docs, scripts, ci]
predicate:
announcement:
note: JSON registry + confidence ladder + staleness · seeded with sipx's generated column only · blocked on X-71
---

# Define the comparison data model and its checker

## Goal

Establish the comparison registry and the check that makes it a measurement rather than a table,
seeded with sipx's own generated column so the mechanism is proven before any external claim exists.

## Acceptance

- [ ] `docs/comparison/` holds `dimensions.json`, `stacks.json`, `observations/<stack-id>.json` and
      `schema/*.schema.json` (JSON Schema 2020-12), plus a `README.md` schema doc following the
      eight-part outline of [`docs/rfc/README.md`](../rfc/README.md) — including a literal
      `## What is checked` list of every way `--check` fails.
- [ ] The key set is **closed**: an unknown key fails, with a targeted hint for `score` explaining
      that a weighted total is refused because it hides the confidence tier behind a number.
- [ ] `scripts/comparison-report.py` generates `docs/comparison.md`; `--check` validates and
      byte-compares and writes nothing. Shape errors are fatal before render. Every message names
      the row, the fault **and the remedy**. Success prints one house-style line, e.g.
      `comparison: 1 stack over N dimensions, every claim evidenced, none stale`.
- [ ] The checker enforces, each with its own failing-first test:
      - `generated` confidence on a non-sipx stack fails;
      - `measured` without a `reproduce` command fails;
      - `assessed` without a `rationale` fails;
      - an observation with zero evidence entries fails;
      - a missing `version_evaluated` or `evaluated_at` fails;
      - an observation older than `max_observation_age_days` fails, and **the message names the
        refresh command**;
      - a dimension with neither an observation nor an explicit `not_evaluated` marker fails, so
        absence is never ambiguous;
      - **sipx's `generated` cells are recomputed from their live sources and must match** — RFC
        count against `docs/rfc/registry.toml`, gate steps against `scripts/gate.py`.
- [ ] **No suppression list, under any name.** The only escape for an unevidenced claim is demotion
      or removal, and the schema doc says so.
- [ ] `scripts/test-comparison-report.py` — `unittest`, `importlib.util` loader with
      `sys.dont_write_bytecode = True`, an `an_observation(**overrides)` factory with a reserved
      fixture identity, one TestCase class per rule. Each rule has all four test kinds: the real
      artifact passes · a reversed fixture produces the *specific* problem · a legitimate row is not
      flagged · the claim reaches the rendered output.
- [ ] Two `gate.py` steps registered with `# X-72:` comments — `comparison tests` under the `gate`
      job, `comparison` under the `docs` job — and the identical commands added to `ci.yml`, so
      `gate.py --check` and `test-gate.py` both stay green.
- [ ] The seeded dataset contains **sipx only**. External subjects are `X-73`.
- [ ] Mutation check recorded in Progress: hand-edit sipx's RFC count in the generated document and
      show `--check` goes red. A guard that passes under the mutation it exists to catch is the
      `X-36` defect.
- [ ] `./scripts/gate.py` green.

## Progress

Implemented 2026-08-04. Everything in Acceptance is satisfied.

- **`docs/comparison/`** holds `dimensions.json` (six chooser-facing questions), `stacks.json`
  (this repository only), `observations/sipx.json`, three JSON Schema 2020-12 documents under
  `schema/`, and a `README.md` following the eight-part outline of `docs/rfc/README.md` with a
  literal `## What is checked` list of all seventeen ways `--check` fails.
- **The key set is closed** in four places — dimension, stack, finding, marker — plus a fifth for
  an evidence entry. Two hints are named rather than generic: `score`, because a weighted total
  hides the tier behind a number nobody can falsify; and a marker carrying any finding key, because
  a row that says both "nobody looked" and what they would have found has two states at once.
- **The generated column cannot be typed.** A `generated` cell states its numbers as `{rule}`
  placeholders and the value is substituted at render time from the live source — RFC registry,
  gate script, transport enum, audio-claims checker, workspace lint table. Declaring a rule with
  no placeholder fails; interpolating an undeclared one fails; interpolating one at any other tier
  fails. Two rules read a sibling checker's success line and **raise rather than render** if it is
  red, following `claimed_codecs()` in `sync-website.py`.
- **The test count was dropped, deliberately**, and the reason is in the schema doc: the roadmap
  and the board both refuse to publish one, and `--check` runs in a CI job that builds no Rust.
  The gate step count answers the same question from a file rather than from a build.
- **`scripts/test-comparison-report.py`** — 48 tests, all passing, one TestCase per rule with the
  four kinds. Fixture identities are `zz-fixture-stack` / `zz-fixture-dimension`: this file is
  outside `COMPARISON_SCOPE`, so a real subject written into it would fail the provenance check,
  the same reason `test-provenance.py` invents its own term. Two structural tests: no subject id
  appears in the checker's source, and the source contains no suppression list under any name.
- **Registered**: `Step("comparison tests", "gate", …)` and `Step("comparison", "docs", …)` in
  `scripts/gate.py` with `# X-72:` comments, and the mirrored `run:` lines in `ci.yml`.
  `./scripts/gate.py --check` reports **35 steps over 19 CI jobs, none unaccounted for**;
  `test-gate.py` 92 tests pass.

**Mutation check.** With the tree green, `72 RFCs` was hand-edited to `99 RFCs` in the generated
document; `--check` went red with `docs/comparison.md is out of date; run
scripts/comparison-report.py` and exit 1. Regenerating restored it.

**A second, unplanned mutation confirmed it end to end.** Adding this story's own two gate steps
took the gate from 33 to 35, and `--check` went red on the stale document without anybody editing
the data — the `{gate-steps}` cell had re-derived itself. That is the mechanism working on a real
change rather than on a contrived one.

Also green: `check-docs-links.py` (531 links), `check-provenance.sh` and `--history`,
`test-provenance.py`.

## Notes
- Blocked on `X-71`: the checker's denylist-scope rule needs the policy boundary to exist first.
- JSON rather than TOML is a deliberate deviation from the RFC registry — this data is
  machine-written by a skill and schema-validated, where the registry is hand-authored. State the
  reason in the schema doc so the inconsistency reads as a decision.
- Follow the family conventions exactly: stdlib only, no shared helper module, `ROOT` constant,
  one `list[str]`-returning predicate per rule, `.get()` everywhere.
