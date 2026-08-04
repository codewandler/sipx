---
name: compare-stacks
description: Derive or refresh the stack comparison dataset under docs/comparison/. Use when comparison-report.py --check reports a stale observation, when a comparison subject is added or removed, or when the comparison page needs refreshing after a subject ships a new release. Clones and pins each subject, derives one observation per dimension under the confidence ladder, and iterates against the checker until clean.
---

# Deriving the stack comparison

The comparison page exists because a capability review was once produced by fanning research
agents over another stack, and the result was **unreproducible**: the method lived in a
conversation, the evidence lived in prose, and nothing could re-run it or say when it went stale.
This file is the fix. A refresh must be a command, not a conversation.

Read [`docs/comparison/README.md`](../../../docs/comparison/README.md) first. It is the schema and
the list of every way the checker fails; this file is the procedure that produces something the
checker accepts.

## The one rule about names

**Never write a subject's name outside `docs/comparison/`.** The provenance check
(`scripts/check-provenance.sh`) permits comparison subjects in that directory, in the
`docs/comparison.md` generated from it, and on the generated public page — nowhere else, and the
commit-message scan has no exception at all.

That means: not in this file, not in a commit message, not in the CHANGELOG, not in a story, not in
a scratch note inside the repository. Read the subject list from `docs/comparison/stacks.json`; do
not carry it in prose. When you commit a refresh, the message says what changed structurally
("refresh three comparison observations") and names nobody.

## Procedure

### 1. Scope

Read `docs/comparison/stacks.json` and `docs/comparison/dimensions.json`. Every stack must answer
every dimension. Run `./scripts/comparison-report.py --check` first and read what it says — on a
refresh it names exactly which observations are stale, and those are the only ones that need
re-deriving. Refreshing a row that is not stale wastes the run and resets its date for no reason.

The same run tells you what is *about* to expire: from thirty days out it prints a `notice:` line
per approaching observation and still exits 0, and the success line always carries a countdown to
the soonest expiry. **Refresh what the notice names, not the whole set.** A dataset whose rows all
expire together produces one red gate demanding every subject be re-derived at once; refreshing
subject by subject as each ages is what pulls the dates apart, and pulled-apart dates are the
desired end state rather than untidiness.

Never edit `evaluated_at` to achieve that. It is the day the evidence was read, and adjusting it to
smooth the schedule is a lie told to a checker.

### 2. Clone and pin

Work outside the repository — the scratch directory, never a path inside the checkout, because a
subject's source tree inside `docs/` would be scanned by every checker in the gate.

For each subject, clone shallow at a **tag**, never at a branch head:

```sh
git clone --depth 1 --branch <tag> <repository> "$SCRATCH/<stack-id>"
```

The tag becomes `version_evaluated`. If the project publishes releases but you pinned a commit,
the observation cannot say which version it is true of, and the whole point of pinning is lost.
If a project has no tags at all, that is itself a finding for the maturity dimension — record it,
and pin the commit sha as `version_evaluated`.

Remove the clones when the run is done. Do not leave them where the next run might read a stale
tree and believe it is current.

### 3. Derive, one agent per dimension group

Fan out in parallel. Give each agent one dimension, the clone path, the dimension's `question` and
`why` from `dimensions.json`, and the per-dimension recipe from
[`references/dimensions.md`](references/dimensions.md). Each returns a candidate observation.

Do **not** give one agent all six dimensions for one subject. The failure mode is a single
narrative about a project that then gets sliced into cells, and narratives flatter or damn a
project as a whole. One agent per question keeps the answers independent.

### 4. Apply the evidence rules

These are rules, not advice. The checker enforces every one of them.

- **Pick the lowest tier the evidence supports, not the highest you can defend.** `assessed` is
  a legitimate answer and the checker accepts it with a rationale; a `measured` claim that nobody
  could actually re-run is a lie with a command attached.
- **`measured` needs a `reproduce` command, and you must run it.** Paste it into a shell against
  the pinned clone and check that its output is what the summary says. A `reproduce` nobody ran is
  a citation that cannot fail, which is the exact defect this repository has closed twice.
- **`documented` cites the subject's own material** — its docs, its release notes, its advisories.
  A third party's blog post about the subject is not the subject documenting itself.
- **`assessed` needs a `rationale` saying what the judgment rests on**, so a reader can disagree
  with it. Keep these in the minority; they are where marketing creeps back in, and the checker
  cannot tell a fair rationale from an unfair one.
- **At least one evidence entry per observation**, each a URL or a repository-relative path, each
  pointing at something that can stop being true. A path must exist. Prose alone is never evidence.
- **`version_evaluated` and `evaluated_at` on every finding.** No exceptions, and no back-dating:
  `evaluated_at` is the day the evidence was actually read.
- **Never write this repository's own cells by hand.** They are `generated`, they carry `{rule}`
  placeholders, and the value is substituted at render time. If a number about this repository
  belongs in a cell and no rule computes it, add a rule to `comparison-report.py` — do not type
  the number.
- **A dimension nobody could evaluate gets a `not_evaluated` marker with a reason**, and nothing
  else. It must not also say what you suspect the answer is.

### 5. Record the evidence asymmetry

Every run must state which subjects were read at source level and which only from published
material. The asymmetry **flatters sipx** — its own column is computed from a repository the author
knows completely, and every other column is somebody reading unfamiliar code under time pressure —
so the page has to say so. Put it in the hand-written surround of the public page, and put the
per-subject detail in the story's Progress.

If a subject was read only from published material, its observations are `documented` or
`assessed`. They are not `measured`, whatever the docs claim.

### 6. Say where this repository loses

At least one dimension must be a row this repository does not win, stated plainly, with evidence.
That is not editorial balance for its own sake — it is the credibility mechanism for every other
row, and a page without it is the marketing artifact `docs/designs/rfc-registry-grain.md` refuses.
A run that produces a clean sweep has not been careful, it has been credulous; go back and check
what the other subjects have that this one does not.

### 7. Emit and iterate

Write `docs/comparison/observations/<stack-id>.json`, then:

```sh
./scripts/comparison-report.py --check
```

Fix what it names and run it again until it is clean. Then regenerate and verify:

```sh
./scripts/comparison-report.py
./scripts/comparison-report.py --check
./scripts/check-provenance.sh
```

Finally run the full gate. The comparison feeds the public site, so a red `docs site` step is this
work's problem even when the failure is a broken link three files away.

## Adding a subject

A new subject is the same procedure with one step in front of it and one warning after it.

1. Append an entry to `docs/comparison/stacks.json`: `id` (kebab-case, and the basename its
   observations file will use), `name`, `language` — the *implementation* language, not the ones it
   has bindings for — `repository` and `license`. Never `is_self`; exactly one stack carries that
   and it is not this one.
2. Derive **all** dimensions for it, not the interesting ones. `--check` fails on any stack and
   dimension pair with neither a finding nor a `not_evaluated` marker, which is the mechanism that
   stops a new subject arriving with only its flattering rows filled in.
3. Its `evaluated_at` will differ from every existing row, and that is correct — see the refresh
   note in step 1 of the procedure above. Do not touch the other rows to match it.

Before adding one, be honest about the cost: every subject is another six observations to re-derive
on every cycle, and a page with too many columns answers nobody's question well. A subject earns
its place by being one a reader is *actually choosing between*, not by existing.

A subject whose source cannot be read at all is a different case. It can hold `documented` rows
from its own published material, but not `measured` ones, and the run must record that asymmetry —
see step 5.

## What this procedure does not do

It does not establish interop. Interop is a property of a test run against another implementation,
and it lives in `tests/interop/`. A comparison row saying two stacks both implement something is
not evidence that they work together.

It does not audit anyone. Reading a repository for a day finds what a project publishes about
itself and what its CI configuration admits; it does not find vulnerabilities, and the page must
not imply that a quiet advisory history means a safe one.

It does not produce a winner. There is no weighted score and no overall verdict — the schema
refuses a `score` key by name, because a single number hides the confidence tier, which is the only
thing making an asymmetric comparison trustworthy.
