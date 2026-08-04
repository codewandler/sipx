# Design: stack comparison

**Status:** accepted · **Pillar:** Build · **Epic:** `stack-comparison` · **Stories:** X-71, X-72,
X-73, X-74, X-97

## Why

The public site tells a reader what sipx is and whether it fits, but never what choosing it costs
or wins against the alternatives they are actually weighing. `does-this-fit.md` answers "is this
for me" in sipx's own terms; it cannot answer "why this and not that", which is the question a
chooser arrives with. A 2026-08-04 capability review produced that answer once, by fanning research
agents out over another stack — and the result was **unreproducible**. The method lived in a
conversation, the evidence lived in prose, and nothing could re-run it or say when it went stale.

The reason this repository does not already have such a page is not oversight. It is doctrine:

> The registry exists to be a *measurement*. Its two rules — generated, and checked — are the whole
> value; a compliance table that is neither is marketing.
> — [`rfc-registry-grain.md`](rfc-registry-grain.md)

and, from the same file, the sentence that most directly indicts a comparison table:

> interop is a property of a test run against another implementation, not of a row in a table.

`X-35` is the scar: hand-maintained public capability tables that sold Opus, bridging and a
DTLS-SRTP path no call could reach. A comparison table is that failure with a larger blast radius,
because half its claims are about software this repository does not control and cannot test.

So the design question is not "what dimensions should we compare" but **"what shape of comparison
survives generated-and-checked when half the subject is external?"**

## Approach

Three rules, in priority order. Everything else follows from them.

**1. sipx's own column is generated, never typed.** RFC count from
[`rfc/registry.toml`](../rfc/registry.toml), gate steps from `scripts/gate.py`, transports from the
feature matrix, test count from the suite. A hand-written number in sipx's column is a checker
failure, exactly as `check-audio-claims.py` treats a hand-written codec claim.

**2. Prefer a re-runnable measurement over a recorded judgment.** For an open-source subject much
is mechanically derivable — fuzz targets present, linter config present, race detection in CI,
torture-corpus cases enabled versus commented out, declared RFCs, published advisories. Such an
observation carries a `reproduce` command and is re-derivable from a pinned tag. Judgment is
permitted, marked, and kept in the minority.

**3. Every observation cites evidence that can stop being true** (`X-43`). At least one evidence
entry, a pinned `version_evaluated`, an `evaluated_at` date. Evidence is a URL or a path in a
cloned tree at a named tag; prose alone is not evidence, here as everywhere else.

The **confidence ladder** is the mechanism that makes an asymmetric comparison honest rather than
merely careful:

| Tier | Means | Who may hold it |
|---|---|---|
| `generated` | computed from this repository at build time | sipx only |
| `measured` | a `reproduce` command re-derives it from the subject at a pinned tag | any open-source subject |
| `documented` | the subject's own docs, release notes or advisories state it | any subject |
| `assessed` | reviewer judgment from indirect evidence | any subject, rendered with a visible marker |

The checker enforces the ladder — `generated` on a non-sipx subject fails, `measured` without
`reproduce` fails, `assessed` without a `rationale` fails — and the published page renders the tier
per cell, so a reader can see which comparisons are strong and which are one person's reading.

**Staleness is a gate failure, not a footnote.** A comparison ages the moment it ships. An
observation past `max_observation_age_days` fails `--check`, and the failure names the command that
refreshes it. This is the `X-34` doctrine in a third place: refuse to report rather than report
falsely.

**Capability ownership is leaf-level.** `X-97` extends the comparison from a small chooser-facing
set of dimensions to a complete public-capability ledger for one immutable subject release. Each
leaf records evidence, ownership, disposition and, when sipx owns an open gap, the story that closes
it. Platform functions owned by the cluster repository are linked there rather than silently copied
into this stack. A broad row such as "SIP" cannot hide an unclassified method or runtime surface.

**The derivation is a skill, not a memory.** `.claude/skills/compare-stacks/` encodes the process —
scope, clone and pin, fan out by dimension group, apply the evidence rules, emit JSON, iterate
against `--check`. A process that lives in an agent's head produced the unreproducible review this
epic exists to replace.

**The page follows the compliance chain, hop for hop**: JSON → `comparison-report.py` →
`docs/comparison.md` → `sync-website.py` inlines a sanitised copy into
`website/docs/reference/comparison.md`. The third hop is not optional — the public-content guard
rejects work-item IDs and internal story links, and our observations will cite both.

**The page must state where sipx loses.** The maturity row — no published crates, no external user,
no third-party audit — is the credibility mechanism for every other row, and a page that only
flatters is precisely the marketing artifact the grain doc refuses.

## What this costs, and the decision it reverses

Naming other stacks in tracked files reverses two standing decisions, recorded in `X-71` rather
than absorbed silently:

- **`AGENTS.md` non-negotiable 1**, which forbids naming a prior-art project anywhere in the
  repository. The relaxation is scoped to comparison subjects in `docs/comparison/` and the
  documents generated from it. Design rationale still cites RFCs and our own specs — vision
  principle 5 is untouched — and **commit messages stay denied**, so `--history` stays clean and
  the failure mode that requires a history rewrite never fires.
- **`X-47`'s acceptance**, which deleted the product-specific migration pages under the criterion
  "no prior-art project names left in the README or public site". Note this is independent of the
  denylist: the peers named in `tests/interop/` were never denylisted, and the site was kept
  vendor-neutral by product decision. That decision is now changed, deliberately.

## Alternatives considered

- **Keep the comparison internal, publish only `does-this-fit`.** The status quo, and it is
  coherent — but it leaves the chooser's actual question unanswered on the one surface where they
  are asking it, and it leaves the derivation unreproducible, which is the defect that motivated
  this epic independently of publication.
- **Anonymised subjects ("a widely-used Go stack").** Rejected: it reads as evasive, and the
  evidence URLs that make an observation falsifiable would carry the name anyway, so the honesty
  mechanism and the anonymity are mutually exclusive.
- **A weighted score or an overall winner.** Rejected. A single number hides the confidence tier,
  which is the only thing making an asymmetric comparison trustworthy, and it is unfalsifiable —
  the property `X-43` exists to forbid.
- **An exceptions file for claims that cannot be evidenced.** Rejected under the standing rule:
  no suppression list under any name. The sanctioned escape is demotion — a lower tier, or dropping
  the row — because both change what the published page says.
- **Automated periodic refresh in CI.** Rejected: the derivation needs judgment, and a scheduled
  job would produce confidently-wrong rows on a timer. A staleness gate that stops a human is the
  honest version.

## Risks and open questions

- **`assessed` rows are where marketing creeps back in.** The checker can require a rationale; it
  cannot judge whether the rationale is fair. Keeping the tier visible on the page and the count
  low is the whole mitigation, and it is a social one.
- **A red gate on a date** will arrive every `max_observation_age_days` with no code change behind
  it. That is the design working, but it must not become the thing people learn to silence — the
  failure message naming the refresh command is what keeps it actionable.
- **Scope creep of the relaxation.** `X-71` states the boundary as a rule the checker enforces
  rather than a sentence to reinterpret, so widening it requires editing a check.
- Whether the first subject set should be one stack or several is left to `X-73`: more subjects
  make a better page and multiply the staleness burden, and that trade is better made with one
  dataset in hand than predicted here.
