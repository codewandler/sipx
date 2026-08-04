# The comparison registry format

This directory is the source of truth for how sipx compares to other stacks.
`scripts/comparison-report.py` generates [`docs/comparison.md`](../comparison.md) from it and,
with `--check`, holds every claim against its evidence. Both run in the gate, and the public
comparison page under `website/docs/reference/` is generated from the report in turn.

This file documents the format — for whoever refreshes a dataset, and for whoever has to decide
whether a row belongs on a public page at all.

## The grain is one observation per stack per dimension

A dimension is a question a chooser actually has; a stack is a subject; an observation is one
subject's answer to one question. There is no finer grain and no coarser one. Every pair must be
filled in, either by a finding or by an explicit `not_evaluated` marker, so a blank cell can never
mean two things.

The decision is enforced: a record carrying a key the schema does not name is a gate failure, and
a pair with no record at all is a gate failure. `score` is refused by name, because a weighted
total hides the confidence tier behind a number — and a number nobody can falsify is exactly the
property this page must not have.

**Why JSON, when the RFC registry is TOML.** The RFC registry is hand-authored by whoever changes
the code, and TOML is comfortable to write by hand. This data is machine-written by a skill,
validated against JSON Schema, and read back by that skill on every refresh. The inconsistency is
deliberate rather than accidental, and stated here so it reads as a decision.

## Schema

Three kinds of file, each with a JSON Schema 2020-12 document under
[`schema/`](schema). **These keys and no others.**

### `dimensions.json`

| Key | Required | Type | Meaning |
|---|---|---|---|
| `id` | yes | string | Stable identity, kebab-case. Observations reference it; renaming one orphans every observation against it. |
| `title` | yes | string | The section heading in the generated document. |
| `question` | yes | string | What the row asks, phrased as the chooser's question rather than as a feature name. |
| `why` | yes | string | Why the question is worth a row. A dimension that cannot say this is a column, not a question. |

### `stacks.json`

| Key | Required | Type | Meaning |
|---|---|---|---|
| `id` | yes | string | Stable identity, kebab-case, and the basename of `observations/<id>.json`. |
| `name` | yes | string | What the project calls itself. |
| `language` | yes | string | The implementation language — not the languages it has bindings for. |
| `repository` | yes | string | Where the evidence was read. A subject with no readable source cannot hold a `measured` observation. |
| `license` | yes | string | |
| `is_self` | no | boolean | True for this repository and no other. Exactly one stack carries it, and only that stack may hold `generated` cells. |

### `observations/<stack-id>.json`

A `stack` key naming the subject, and an `observations` list. Each entry is either a **finding** or
a **marker**.

A finding:

| Key | Required | Type | Meaning |
|---|---|---|---|
| `dimension` | yes | string | Must name a dimension. |
| `confidence` | yes | string | See the ladder below. |
| `summary` | yes | string | The finding, in one table cell. On a `generated` cell, every computed value is a `{rule}` placeholder — see below. |
| `evidence` | yes | list | At least one citation. Each carries a `note` and exactly one of `url` or `path`; a `path` is repository-relative and must exist. |
| `version_evaluated` | yes | string | The tag, release or version the observation was taken at. A finding with no version has no subject. |
| `evaluated_at` | yes | string | ISO `YYYY-MM-DD`. Past the age limit the check fails. |
| `reproduce` | no | string | Required by the `measured` tier. A command that re-derives the finding at `version_evaluated`. |
| `rationale` | no | string | Required by the `assessed` tier. What the judgment rests on, so a reader can disagree with it. |
| `generated_from` | no | list | Required by, and restricted to, the `generated` tier. Each rule is recomputed at render time and substituted into its placeholder. |

A marker carries `dimension` and `not_evaluated` — a non-empty reason — **and nothing else**. A
marker that also carried a summary would give the row two states at once, which is the ambiguity
the marker exists to remove.

### The confidence ladder

| Tier | Means | Who may hold it |
|---|---|---|
| `generated` | Computed from this repository at render time | this repository only |
| `measured` | A `reproduce` command re-derives it from the subject at the version named | any subject whose source can be read |
| `documented` | The subject's own documentation, release notes or advisories state it | any subject |
| `assessed` | Reviewer judgment from indirect evidence | any subject, and kept in the minority |

The ladder is what makes an asymmetric comparison honest rather than merely careful. Half of this
page is about software this repository does not control and cannot test, and a reader needs to see
which cells are measurements and which are one person's reading — per cell, without leaving the
page.

## What is checked

`comparison-report.py --check` fails on any of:

- a record whose keys are not exactly the schema above (missing required, or unknown), with a
  named hint for `score` and for a marker that also states a finding;
- an unknown `confidence` tier;
- `generated` confidence on a stack not marked `is_self`;
- `measured` with no `reproduce` command;
- `assessed` with no `rationale`;
- `generated_from` at any tier other than `generated`;
- a `generated` cell declaring a rule it has no `{rule}` placeholder for, or interpolating a
  placeholder it did not declare, or naming a rule that does not exist;
- any cell outside the `generated` tier interpolating one of this repository's computed values;
- an observation citing no evidence;
- an evidence entry naming neither `url` nor `path`, or naming both;
- an evidence `path` that does not exist in this repository;
- a missing `version_evaluated`;
- a missing, unparseable, or too-old `evaluated_at` — the staleness message names the refresh
  command;
- an observation filed against a stack or a dimension that is not declared;
- a stack answering the same dimension twice;
- a stack and dimension pair with neither a finding nor a `not_evaluated` marker;
- `stacks.json` marking anything other than exactly one stack `is_self`;
- `docs/comparison.md` differing from what the script would generate.

### The generated column is never typed

sipx's own numbers are not written into the data. The summary carries a placeholder — `{rfc-count}`,
`{gate-steps}`, `{transports}`, `{codecs}`, `{unsafe-policy}` — and the value is substituted at
render time from the live source: the RFC registry, the gate script, the transport enum, the audio
claims checker, the workspace lint table. A hand-written number in this repository's column is
therefore not merely wrong, it is unrepresentable, and editing one into the generated document
fails the byte-compare.

Two of those rules read another checker's success line rather than the underlying file, and if that
checker is red this script **raises rather than rendering**. A comparison page must not become a
second opinion about a fact that already has an owner.

The rules stop short of a test count, deliberately. The [roadmap](../roadmap.md#status) and the
[board](../stories/README.md) both refuse to publish one, because a stated count drifted through
four releases; and `--check` runs in a CI job that builds no Rust, so computing one honestly would
mean running the suite to render a document. The gate step count answers the same question — how
much has to be true before a change lands — from a file rather than from a build.

### Staleness is a failure, not a footnote

A comparison ages the moment it ships. An observation older than `MAX_OBSERVATION_AGE_DAYS` fails
`--check`, and the failure names the command that refreshes it. This will produce a red gate on a
date with no code change behind it. That is the design working — but it is also the failure most
likely to be silenced, which is why the message is actionable and why the limit is a constant in
the script rather than a field in the data.

**The wall has a notice period, and the two are different things.** From `STALE_WARNING_DAYS` out,
every run prints a `notice:` line per observation approaching its limit and **still exits 0**; past
the limit the same row becomes a failure. The notice is not returned by `check()`, deliberately —
that function returns failures, and a warning folded into it would either fail the build a month
early or teach a reader that some of what it returns is advisory. The countdown to the soonest
expiry also rides on the success line, so "when does this need refreshing" is answerable from any
green run rather than only from the one that is about to go red.

The first dataset was derived in one sitting, so every observation in it expires on the same day.
That is not a state to preserve: refreshing subjects one at a time as each approaches its limit
makes the dates diverge, which turns one unrefusable red gate into a series of small ones. The
dates are never edited to achieve that — `evaluated_at` is when the evidence was read, and moving
it to smooth the cliff would be a lie told to a checker.

**There is no suppression list**, under any name. The only way past a rule is demotion to a lower
tier or removal of the row, because both change what the published page says.

It deliberately does **not** verify that an `assessed` rationale is fair, or that the evidence a
row cites has anything to do with the question its dimension asks. Only a reader can. Nor is it an
interop result: interop is a property of a test run against another implementation, not of a row in
a table, and `tests/interop/` is where that claim lives.

`scripts/test-comparison-report.py` tests the checker itself.

## Adding or refreshing a dataset

Do not hand-write `observations/<stack>.json`. Run the `compare-stacks` skill, which clones and
pins each subject, derives the observations, and iterates against `--check` until clean. Then run
`./scripts/comparison-report.py` to regenerate the report. Never hand-edit
[`docs/comparison.md`](../comparison.md).

Naming a comparison subject is permitted **in this directory, in the generated report, and on the
generated public page — nowhere else**. That is the whole of the exception in `AGENTS.md`
non-negotiable 1, and it does not extend to the checker, the skill, the tests, the CHANGELOG or
any commit message.

## What a reader may rely on

- **`id` identifies exactly one record**, for both stacks and dimensions, so a reference resolves
  unambiguously.
- **The key sets are closed and enforced**, so a record cannot quietly gain or lose a field.
- **Every cell states its tier**, and the tier vocabulary is stable: adding a tier or changing what
  one means is a change to how the whole page should be read, and belongs in the CHANGELOG.
- **Every finding is pinned**, to a version and a date, so "which version is this true of?" always
  has an answer.

What is not promised is that a given observation stays true — that is the point of the staleness
gate, and it is why every row carries the version it was taken at.
